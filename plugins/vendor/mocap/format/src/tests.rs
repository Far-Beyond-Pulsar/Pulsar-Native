use super::signal::*;
use super::*;

fn rig() -> RigProfile {
    RigProfile::humanoid(
        "test",
        &[("hips", None), ("spine", Some(0)), ("head", Some(1))],
        &["jawOpen", "blinkL"],
    )
}

fn take(frames: usize) -> Take {
    let mut take = Take::new(TakeMeta::new("walk", 60, 1), rig());
    let columns = take.rig.columns();
    for f in 0..frames {
        let t = f as f32 / 60.0;
        let frame: Vec<f32> = (0..columns).map(|c| (t * (c as f32 + 1.0)).sin()).collect();
        take.push_frame(&frame);
    }
    take.markers.push(Marker { frame: 3, label: "clap".into() });
    take
}

#[test]
fn rig_columns_follow_channel_kinds() {
    let rig = rig();
    // hips position (3) + three rotations (12) + two scalars.
    assert_eq!(rig.columns(), 3 + 12 + 2);
    assert_eq!(rig.column_offsets()[1], 3);
    assert!(rig.validate().is_ok());
}

#[test]
fn rig_rejects_a_parent_that_does_not_precede() {
    let mut rig = rig();
    rig.joints[0].parent = Some(2);
    assert!(matches!(rig.validate(), Err(FormatError::InvalidRig(_))));
}

#[test]
fn varints_round_trip_the_extremes() {
    for v in [0u64, 1, 127, 128, 300, u32::MAX as u64, u64::MAX] {
        let mut buf = Vec::new();
        codec::put_varint(&mut buf, v);
        let mut pos = 0;
        assert_eq!(codec::get_varint(&buf, &mut pos), Some(v));
        assert_eq!(pos, buf.len());
    }
    for v in [0i64, -1, 1, i64::MIN, i64::MAX] {
        assert_eq!(codec::unzigzag(codec::zigzag(v)), v);
    }
}

#[test]
fn a_take_round_trips_bit_exact() {
    let original = take(250);
    let bytes = write_take(&original, 64).expect("write");
    let (read, report) = read_take(&bytes).expect("read");
    assert!(report.is_clean());
    assert_eq!(read.meta, original.meta);
    assert_eq!(read.markers, original.markers);
    assert_eq!(read.frame_count(), 250);
    let lossless = original.rig.channel_index("spine.rotation").expect("channel");
    let first = original.rig.column_offsets()[lossless];
    for c in first..first + 4 {
        assert_eq!(read.columns[c], original.columns[c], "column {c} is lossless");
    }
}

#[test]
fn quantised_channels_stay_within_their_tolerance() {
    let original = take(200);
    let (read, _) = read_take(&write_take(&original, 50).expect("write")).expect("read");
    let shape = original.rig.channel_index("blend.jawOpen").expect("channel");
    let c = original.rig.column_offsets()[shape];
    let range = 2.0; // sin spans [-1, 1]
    for (a, b) in original.columns[c].iter().zip(&read.columns[c]) {
        assert!((a - b).abs() <= range / 65535.0 + 1e-6, "{a} vs {b}");
    }
}

#[test]
fn gaps_survive_as_nan() {
    let mut original = take(20);
    for v in &mut original.columns[0][5..9] {
        *v = f32::NAN;
    }
    let (read, _) = read_take(&write_take(&original, 8).expect("write")).expect("read");
    assert!(read.columns[0][5..9].iter().all(|v| v.is_nan()));
    assert_eq!(read.columns[0][4], original.columns[0][4]);
}

#[test]
fn a_crashed_take_reads_up_to_its_last_whole_block() {
    let original = take(300);
    let bytes = write_take(&original, 100).expect("write");
    let file = TakeFile::open(&bytes).expect("open");
    assert_eq!(file.blocks.len(), 3);
    // The index chunk is 12 + 12 + 3 * 20 bytes; cut it and 10 bytes of the last block.
    let cut = &bytes[..bytes.len() - 94];
    let (read, report) = read_take(cut).expect("a cut take still opens");
    assert!(report.truncated);
    assert_eq!(read.frame_count(), 200);
    assert_eq!(read.columns[0][199], original.columns[0][199]);
}

#[test]
fn a_flipped_byte_loses_one_block_not_the_take() {
    let original = take(300);
    let mut bytes = write_take(&original, 100).expect("write");
    let file = TakeFile::open(&bytes).expect("open");
    let second = file.blocks[1];
    drop(file);
    let at = bytes
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == b"BLCK")
        .nth(1)
        .map(|(i, _)| i + 12 + 20)
        .expect("second block");
    bytes[at] ^= 0xff;
    let (read, report) = read_take(&bytes).expect("read");
    assert_eq!(report.bad_ranges, vec![(second.first_frame, second.frames)]);
    assert!(read.columns[0][150].is_nan());
    assert_eq!(read.columns[0][50], original.columns[0][50]);
}

#[test]
fn a_newer_major_version_is_refused() {
    let mut bytes = write_take(&take(4), 4).expect("write");
    bytes[4] = 9;
    assert!(matches!(TakeFile::open(&bytes), Err(FormatError::UnsupportedVersion { .. })));
    assert!(matches!(TakeFile::open(b"nope"), Err(FormatError::Truncated(_))));
    assert!(matches!(TakeFile::open(b"RIFFxxxxxxxx"), Err(FormatError::BadMagic)));
}

#[test]
fn the_writer_streams_and_rejects_a_wrong_width() {
    let rig = rig();
    let meta = TakeMeta::new("s", 30, 1);
    let mut writer = TakeWriter::new(Vec::new(), &meta, &rig, 4).expect("writer");
    assert!(matches!(writer.push_frame(&[0.0]), Err(FormatError::FrameWidth { .. })));
    let frame = vec![0.5; rig.columns()];
    for _ in 0..10 {
        writer.push_frame(&frame).expect("frame");
    }
    writer.add_marker(&Marker { frame: 2, label: "m".into() }).expect("marker");
    assert_eq!(writer.frames(), 10);
    let bytes = writer.finish().expect("finish");
    let (read, report) = read_take(&bytes).expect("read");
    assert!(report.is_clean());
    assert_eq!(read.frame_count(), 10);
    assert_eq!(read.markers.len(), 1);
}

#[test]
fn compression_beats_raw_on_a_smooth_take() {
    let t = take(600);
    let bytes = write_take(&t, 120).expect("write");
    let raw = t.columns.len() * t.frame_count() * 4;
    assert!(bytes.len() < raw, "{} bytes vs {raw} raw", bytes.len());
}

#[test]
fn one_euro_smooths_noise_and_follows_a_step() {
    let mut filter = OneEuro::new(1.0, 0.5);
    let mut out = 0.0;
    for i in 0..120 {
        let noise = if i % 2 == 0 { 0.05 } else { -0.05 };
        out = filter.filter(noise, 1.0 / 60.0);
    }
    assert!(out.abs() < 0.03, "noise is damped: {out}");
    for _ in 0..60 {
        out = filter.filter(1.0, 1.0 / 60.0);
    }
    assert!(out > 0.9, "a step is followed: {out}");
    assert!(filter.filter(f32::NAN, 0.016).is_nan());
}

#[test]
fn quaternion_filter_stays_unit_and_on_one_hemisphere() {
    let mut filter = QuatFilter::new(2.0, 0.1);
    let a = filter.filter([0.0, 0.0, 0.0, 1.0], 1.0 / 60.0);
    let b = filter.filter([0.0, 0.0, 0.0, -1.0], 1.0 / 60.0);
    for q in [a, b] {
        let len: f32 = q.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((len - 1.0).abs() < 1e-5);
    }
    assert!(b[3] > 0.9, "the flipped sign was aligned: {b:?}");
}

#[test]
fn gaps_fill_only_when_short_and_bounded() {
    let mut col = [0.0, f32::NAN, f32::NAN, 3.0, f32::NAN, f32::NAN, f32::NAN, 7.0, f32::NAN];
    let filled = fill_gaps(&mut col, 2);
    assert_eq!(filled, 2);
    assert_eq!(&col[..4], &[0.0, 1.0, 2.0, 3.0]);
    assert!(col[4].is_nan(), "a gap longer than the limit stays");
    assert!(col[8].is_nan(), "a trailing gap has no right neighbour");
}

#[test]
fn sampling_interpolates_and_reports_outside() {
    let col = [0.0, 10.0, 20.0];
    assert_eq!(sample_linear(&col, 0.5), 5.0);
    assert!(sample_linear(&col, 2.5).is_nan());
    assert!(sample_linear(&col, -0.1).is_nan());
}
