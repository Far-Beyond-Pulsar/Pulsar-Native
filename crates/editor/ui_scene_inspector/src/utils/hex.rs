//! The byte viewer's model: how bytes fall into rows, which field a byte
//! belongs to, and what the bytes under the cursor read as.
//!
//! The bytes are records of one length (one component value each; a single
//! value is one record). Every record starts a row of its own, so a column of
//! a thousand values reads as a thousand aligned blocks, never as a stream
//! that drifts across the grid.

/// A named range of a record, from its type's layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Region {
    pub name: String,
    pub type_label: String,
    pub offset: usize,
    pub len: usize,
}

/// The regions that lie in a record of `size` bytes, in offset order and
/// without overlap (an earlier region wins).
pub fn normalize_regions(mut regions: Vec<Region>, size: usize) -> Vec<Region> {
    regions.sort_by_key(|region| region.offset);
    let mut end = 0;
    regions.retain(|region| {
        let fits = region.len > 0
            && region.offset >= end
            && region
                .offset
                .checked_add(region.len)
                .is_some_and(|last| last <= size);
        if fits {
            end = region.offset + region.len;
        }
        fits
    });
    regions
}

/// The region containing `offset`; regions as [`normalize_regions`] left
/// them. A byte in none is padding, or a part its type does not describe.
pub fn region_at(regions: &[Region], offset: usize) -> Option<usize> {
    let after = regions.partition_point(|region| region.offset <= offset);
    let index = after.checked_sub(1)?;
    (offset < regions[index].offset + regions[index].len).then_some(index)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HexLayout {
    pub record_len: usize,
    pub records: usize,
    pub per_row: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HexRow {
    pub record: usize,
    /// Of the row's first byte within its record.
    pub offset: usize,
    /// Bytes in the row (the record's last row may be short).
    pub len: usize,
}

impl HexLayout {
    pub fn new(record_len: usize, records: usize, per_row: usize) -> Self {
        Self {
            record_len,
            records,
            per_row: per_row.max(1),
        }
    }

    pub fn len(&self) -> usize {
        self.record_len * self.records
    }

    pub fn rows_per_record(&self) -> usize {
        self.record_len.div_ceil(self.per_row)
    }

    pub fn rows(&self) -> usize {
        self.rows_per_record() * self.records
    }

    pub fn row(&self, row: usize) -> Option<HexRow> {
        if row >= self.rows() {
            return None;
        }
        let per_record = self.rows_per_record();
        let offset = (row % per_record) * self.per_row;
        Some(HexRow {
            record: row / per_record,
            offset,
            len: self.per_row.min(self.record_len - offset),
        })
    }

    /// The row holding the byte at `index` of the whole run.
    pub fn row_of(&self, index: usize) -> usize {
        if self.record_len == 0 {
            return 0;
        }
        (index / self.record_len) * self.rows_per_record()
            + (index % self.record_len) / self.per_row
    }

    /// Where a cursor at `index` lands after moving by whole bytes.
    pub fn step(&self, index: usize, bytes: isize) -> usize {
        index
            .saturating_add_signed(bytes)
            .min(self.len().saturating_sub(1))
    }

    /// Where a cursor lands moving by rows, keeping its column where the
    /// target row has one.
    pub fn step_rows(&self, index: usize, rows: isize) -> usize {
        if self.len() == 0 {
            return 0;
        }
        let from = self.row_of(index);
        let to = from.saturating_add_signed(rows).min(self.rows() - 1);
        let (Some(source), Some(target)) = (self.row(from), self.row(to)) else {
            return index;
        };
        let column = (index % self.record_len) - source.offset;
        target.record * self.record_len + target.offset + column.min(target.len - 1)
    }

    /// The first and last byte of `index`'s row.
    pub fn row_span(&self, index: usize) -> (usize, usize) {
        match self.row(self.row_of(index)) {
            Some(row) => {
                let start = row.record * self.record_len + row.offset;
                (start, start + row.len - 1)
            }
            None => (index, index),
        }
    }
}

/// What the bytes starting at the cursor read as, each as far as there are
/// bytes for it.
pub fn interpret(bytes: &[u8], big_endian: bool) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    let Some(&first) = bytes.first() else {
        return out;
    };
    macro_rules! number {
        ($label:literal, $ty:ty) => {
            if let Some(raw) = bytes
                .get(..std::mem::size_of::<$ty>())
                .and_then(|slice| slice.try_into().ok())
            {
                let value = if big_endian {
                    <$ty>::from_be_bytes(raw)
                } else {
                    <$ty>::from_le_bytes(raw)
                };
                out.push(($label, format!("{value:?}")));
            }
        };
    }
    out.push(("u8", first.to_string()));
    out.push(("i8", (first as i8).to_string()));
    number!("u16", u16);
    number!("i16", i16);
    number!("u32", u32);
    number!("i32", i32);
    number!("f32", f32);
    number!("u64", u64);
    number!("i64", i64);
    number!("f64", f64);
    out.push(("bits", format!("{:04b} {:04b}", first >> 4, first & 0x0f)));
    out.push((
        "char",
        if (0x20..0x7f).contains(&first) {
            format!("'{}'", first as char)
        } else {
            "-".to_string()
        },
    ));
    out
}

pub fn printable(byte: u8) -> char {
    if (0x20..0x7f).contains(&byte) {
        byte as char
    } else {
        '.'
    }
}

/// Bytes as space-separated hex pairs.
pub fn hex_string(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A classic dump: offset, hex, text.
pub fn dump(bytes: &[u8], per_row: usize) -> String {
    let per_row = per_row.max(1);
    let mut out = String::new();
    for (row, chunk) in bytes.chunks(per_row).enumerate() {
        let text: String = chunk.iter().map(|&byte| printable(byte)).collect();
        out.push_str(&format!(
            "{:08x}  {:<width$}  |{text}|\n",
            row * per_row,
            hex_string(chunk),
            width = per_row * 3 - 1
        ));
    }
    out
}

/// A typed offset: `31`, `0x1f` or `1fh`.
pub fn parse_offset(text: &str) -> Option<usize> {
    let text = text.trim().to_lowercase();
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_suffix('h')) {
        return usize::from_str_radix(hex, 16).ok();
    }
    text.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_record_starts_its_own_row() {
        let layout = HexLayout::new(20, 3, 16);
        assert_eq!((layout.rows_per_record(), layout.rows()), (2, 6));
        assert_eq!(
            layout.row(1),
            Some(HexRow {
                record: 0,
                offset: 16,
                len: 4
            })
        );
        assert_eq!(layout.row(2).unwrap().record, 1);
        assert_eq!(layout.row(6), None);
        for index in 0..layout.len() {
            let row = layout.row(layout.row_of(index)).unwrap();
            let start = row.record * 20 + row.offset;
            assert!((start..start + row.len).contains(&index));
        }
    }

    #[test]
    fn the_cursor_keeps_its_column_and_stays_inside() {
        let layout = HexLayout::new(20, 2, 16);
        // Column 10 into a 4-byte row clamps to its last byte.
        assert_eq!(layout.step_rows(10, 1), 19);
        // And down into the next record's first row.
        assert_eq!(layout.step_rows(19, 1), 23);
        assert_eq!(layout.step_rows(10, -5), 10);
        assert_eq!(layout.step(39, 5), 39);
        assert_eq!(layout.step(0, -1), 0);
        assert_eq!(layout.row_span(18), (16, 19));
        let empty = HexLayout::new(0, 4, 16);
        assert_eq!((empty.rows(), empty.step_rows(0, 1)), (0, 0));
    }

    fn region(name: &str, offset: usize, len: usize) -> Region {
        Region {
            name: name.into(),
            type_label: String::new(),
            offset,
            len,
        }
    }

    #[test]
    fn regions_outside_or_overlapping_are_dropped_and_gaps_have_none() {
        let regions = normalize_regions(
            vec![
                region("b", 8, 4),
                region("a", 0, 4),
                region("overlap", 2, 4),
                region("outside", 14, 4),
                region("empty", 5, 0),
            ],
            16,
        );
        let names: Vec<&str> = regions.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
        assert_eq!(region_at(&regions, 3), Some(0));
        assert_eq!(region_at(&regions, 4), None);
        assert_eq!(region_at(&regions, 11), Some(1));
        assert_eq!(region_at(&regions, 12), None);
    }

    #[test]
    fn bytes_read_both_ways_round_and_only_as_far_as_they_go() {
        let bytes = 1.5f32.to_le_bytes();
        let read = interpret(&bytes, false);
        let get = |label: &str| read.iter().find(|(l, _)| *l == label).map(|(_, v)| v.clone());
        assert_eq!(get("f32").as_deref(), Some("1.5"));
        assert_eq!(get("u32").as_deref(), Some("1069547520"));
        assert_eq!(get("u64"), None);
        let big = interpret(&[0x01, 0x02], true);
        assert!(big.contains(&("u16", "258".to_string())));
        assert!(interpret(&[], false).is_empty());
    }

    #[test]
    fn dumps_and_offsets_are_written_the_usual_way() {
        assert_eq!(dump(b"AB\0", 4), "00000000  41 42 00     |AB.|\n");
        assert_eq!(parse_offset("0x1F"), Some(31));
        assert_eq!(parse_offset("1fh"), Some(31));
        assert_eq!(parse_offset(" 31 "), Some(31));
        assert_eq!(parse_offset("zz"), None);
    }
}
