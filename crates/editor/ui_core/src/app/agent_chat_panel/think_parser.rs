//! Incremental `<think>` / `</think>` splitter and chunk batcher for streamed
//! model output.
//!
//! Owns [`ThinkParser`], the per-request-iteration state machine that turns raw
//! provider text chunks into [`ThinkEvent`]s (prose, thinking start, thinking
//! text, thinking end), batching small fragments so the UI is not flooded.
//! Entry points: [`ThinkParser::push_chunk`] for each streamed chunk and
//! [`ThinkParser::finish`] once the stream ends. It has no GPUI or channel
//! dependency: callers supply the clock (`now`) and an emit callback, which
//! keeps it unit testable.

use std::time::{Duration, Instant};

const THINK_OPEN: &str = "<think>";
const THINK_CLOSE: &str = "</think>";

/// Prose is flushed once this many bytes are buffered, on any newline, or
/// after `CHUNK_BATCH_INTERVAL`.
const CHUNK_BATCH_BYTES: usize = 256;
/// Maximum age of buffered prose before it is flushed.
const CHUNK_BATCH_INTERVAL: Duration = Duration::from_millis(24);
/// Thinking text is flushed once this many bytes are buffered, or after
/// `THINK_BATCH_INTERVAL`.
const THINK_BATCH_BYTES: usize = 128;
/// Maximum age of buffered thinking text before it is flushed.
const THINK_BATCH_INTERVAL: Duration = Duration::from_millis(30);

/// One piece of parsed output, in the order it must reach the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ThinkEvent {
    /// Prose text outside any think block.
    Chunk(String),
    /// A `<think>` tag was opened.
    ThinkingStarted,
    /// Text inside a think block.
    ThinkingChunk(String),
    /// The think block was closed (explicitly or because the stream ended).
    ThinkingDone,
}

/// Parser state for one streamed provider response.
pub(super) struct ThinkParser {
    pending_chunk: String,
    pending_thinking: String,
    last_emit: Instant,
    last_think_emit: Instant,
    in_thinking: bool,
}

impl ThinkParser {
    /// Create a parser whose batching timers start at `now`.
    pub(super) fn new(now: Instant) -> Self {
        Self {
            pending_chunk: String::new(),
            pending_thinking: String::new(),
            last_emit: now,
            last_think_emit: now,
            in_thinking: false,
        }
    }

    /// Feed one streamed chunk; `emit` receives every event that is ready.
    /// Text that is not yet worth sending stays buffered until a later
    /// `push_chunk` or [`finish`](Self::finish).
    pub(super) fn push_chunk(
        &mut self,
        chunk: &str,
        now: Instant,
        emit: &mut impl FnMut(ThinkEvent),
    ) {
        let mut rest: &str = chunk;
        loop {
            if self.in_thinking {
                if let Some(end) = rest.find(THINK_CLOSE) {
                    self.pending_thinking.push_str(&rest[..end]);
                    if !self.pending_thinking.is_empty() {
                        emit(ThinkEvent::ThinkingChunk(std::mem::take(
                            &mut self.pending_thinking,
                        )));
                    }
                    emit(ThinkEvent::ThinkingDone);
                    self.in_thinking = false;
                    rest = &rest[end + THINK_CLOSE.len()..];
                    if rest.is_empty() {
                        break;
                    }
                } else {
                    self.pending_thinking.push_str(rest);
                    // Emit thinking in small batches so the block fills progressively.
                    let should_emit = self.pending_thinking.len() >= THINK_BATCH_BYTES
                        || now.saturating_duration_since(self.last_think_emit)
                            >= THINK_BATCH_INTERVAL;
                    if should_emit {
                        emit(ThinkEvent::ThinkingChunk(std::mem::take(
                            &mut self.pending_thinking,
                        )));
                        self.last_think_emit = now;
                    }
                    break;
                }
            } else if let Some(start) = rest.find(THINK_OPEN) {
                let before = &rest[..start];
                if !before.is_empty() {
                    self.pending_chunk.push_str(before);
                    emit(ThinkEvent::Chunk(std::mem::take(&mut self.pending_chunk)));
                    self.last_emit = now;
                }
                emit(ThinkEvent::ThinkingStarted);
                self.in_thinking = true;
                rest = &rest[start + THINK_OPEN.len()..];
            } else {
                self.pending_chunk.push_str(rest);
                let should_emit = self.pending_chunk.len() >= CHUNK_BATCH_BYTES
                    || self.pending_chunk.contains('\n')
                    || now.saturating_duration_since(self.last_emit) >= CHUNK_BATCH_INTERVAL;
                if should_emit {
                    emit(ThinkEvent::Chunk(std::mem::take(&mut self.pending_chunk)));
                    self.last_emit = now;
                }
                break;
            }
        }
    }

    /// Flush buffered text after the stream ended. A think block that was
    /// never closed is closed here so the UI does not show it as running.
    pub(super) fn finish(&mut self, emit: &mut impl FnMut(ThinkEvent)) {
        if !self.pending_chunk.is_empty() {
            emit(ThinkEvent::Chunk(std::mem::take(&mut self.pending_chunk)));
        }
        // Stream ended mid-think: flush remaining content then close.
        if self.in_thinking {
            if !self.pending_thinking.is_empty() {
                emit(ThinkEvent::ThinkingChunk(std::mem::take(
                    &mut self.pending_thinking,
                )));
            }
            emit(ThinkEvent::ThinkingDone);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(chunks: &[&str]) -> Vec<ThinkEvent> {
        let t0 = Instant::now();
        let mut parser = ThinkParser::new(t0);
        let mut out = Vec::new();
        for chunk in chunks {
            parser.push_chunk(chunk, t0, &mut |e| out.push(e));
        }
        parser.finish(&mut |e| out.push(e));
        out
    }

    #[test]
    fn plain_text_is_buffered_until_finish() {
        let events = run(&["hello", " world"]);
        assert_eq!(events, vec![ThinkEvent::Chunk("hello world".into())]);
    }

    #[test]
    fn newline_flushes_immediately() {
        let t0 = Instant::now();
        let mut parser = ThinkParser::new(t0);
        let mut out = Vec::new();
        parser.push_chunk("line one\n", t0, &mut |e| out.push(e));
        assert_eq!(out, vec![ThinkEvent::Chunk("line one\n".into())]);
    }

    #[test]
    fn large_chunk_flushes_at_byte_threshold() {
        let t0 = Instant::now();
        let mut parser = ThinkParser::new(t0);
        let mut out = Vec::new();
        let big = "x".repeat(CHUNK_BATCH_BYTES);
        parser.push_chunk(&big, t0, &mut |e| out.push(e));
        assert_eq!(out, vec![ThinkEvent::Chunk(big)]);
    }

    #[test]
    fn elapsed_interval_flushes_small_chunk() {
        let t0 = Instant::now();
        let mut parser = ThinkParser::new(t0);
        let mut out = Vec::new();
        parser.push_chunk("a", t0, &mut |e| out.push(e));
        assert!(out.is_empty());
        parser.push_chunk("b", t0 + CHUNK_BATCH_INTERVAL, &mut |e| out.push(e));
        assert_eq!(out, vec![ThinkEvent::Chunk("ab".into())]);
    }

    #[test]
    fn think_block_inside_one_chunk() {
        let events = run(&["before<think>idea</think>after"]);
        assert_eq!(
            events,
            vec![
                ThinkEvent::Chunk("before".into()),
                ThinkEvent::ThinkingStarted,
                ThinkEvent::ThinkingChunk("idea".into()),
                ThinkEvent::ThinkingDone,
                ThinkEvent::Chunk("after".into()),
            ]
        );
    }

    #[test]
    fn think_block_ending_a_chunk_emits_nothing_after() {
        let events = run(&["<think>idea</think>"]);
        assert_eq!(
            events,
            vec![
                ThinkEvent::ThinkingStarted,
                ThinkEvent::ThinkingChunk("idea".into()),
                ThinkEvent::ThinkingDone,
            ]
        );
    }

    #[test]
    fn thinking_text_is_batched_across_chunks() {
        let events = run(&["<think>ab", "cd</think>ok"]);
        assert_eq!(
            events,
            vec![
                ThinkEvent::ThinkingStarted,
                ThinkEvent::ThinkingChunk("abcd".into()),
                ThinkEvent::ThinkingDone,
                ThinkEvent::Chunk("ok".into()),
            ]
        );
    }

    #[test]
    fn thinking_flushes_at_byte_threshold() {
        let t0 = Instant::now();
        let mut parser = ThinkParser::new(t0);
        let mut out = Vec::new();
        parser.push_chunk("<think>", t0, &mut |e| out.push(e));
        let big = "t".repeat(THINK_BATCH_BYTES);
        parser.push_chunk(&big, t0, &mut |e| out.push(e));
        assert_eq!(
            out,
            vec![ThinkEvent::ThinkingStarted, ThinkEvent::ThinkingChunk(big)]
        );
    }

    #[test]
    fn unterminated_think_is_closed_on_finish() {
        let events = run(&["<think>never closed"]);
        assert_eq!(
            events,
            vec![
                ThinkEvent::ThinkingStarted,
                ThinkEvent::ThinkingChunk("never closed".into()),
                ThinkEvent::ThinkingDone,
            ]
        );
    }

    #[test]
    fn empty_think_block_emits_start_and_done_only() {
        let events = run(&["<think></think>"]);
        assert_eq!(
            events,
            vec![ThinkEvent::ThinkingStarted, ThinkEvent::ThinkingDone]
        );
    }
}
