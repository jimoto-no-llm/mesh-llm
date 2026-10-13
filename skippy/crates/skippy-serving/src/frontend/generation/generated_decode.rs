//! Incremental UTF-8 assembly for generated tokens.
//!
//! Each token used to rescan and copy the whole decoded prefix. This keeps the
//! uncommitted byte tail, appends only the newly valid suffix, and leaves stop
//! search, invalid UTF-8, and the safe streaming holdback on their old rules.

use crate::frontend::stop_sequences::stop_in_new_text;
use crate::frontend::util::trim_at_stop;

pub(super) struct GeneratedUtf8 {
    pending: Vec<u8>,
    decoded: String,
    /// A hard UTF-8 error already closed the valid prefix. Later bytes cannot
    /// extend it, matching `str::from_utf8` stopping at the first error.
    closed: bool,
}

pub(super) struct OpenText {
    text: String,
    streamed_text_len: usize,
    max_stop_bytes: usize,
    /// Length of `text` already searched for stop sequences.
    stop_searched_len: usize,
    /// `text.len()` last time `text` was the decoded prefix. A shorter `text`
    /// means a stop trim (or another rewrite) and takes the old divergence path.
    synced_len: usize,
    decoder: GeneratedUtf8,
    #[cfg(test)]
    bytes_copied: usize,
}

pub(super) enum EmitPlan {
    Idle,
    Shrink(usize),
    Delta { start: usize, end: usize },
}

impl GeneratedUtf8 {
    fn new() -> Self {
        Self {
            pending: Vec::new(),
            decoded: String::new(),
            closed: false,
        }
    }

    fn push(&mut self, piece: &[u8]) {
        if piece.is_empty() || self.closed {
            return;
        }
        self.pending.extend_from_slice(piece);
        let prefix = crate::frontend::util::utf8_prefix(&self.pending);
        let valid_len = prefix.valid_len;
        let hard_error = prefix.hard_error;
        if valid_len > 0
            && let Ok(valid) = std::str::from_utf8(&self.pending[..valid_len])
        {
            self.decoded.push_str(valid);
            self.pending.drain(..valid_len);
        }
        if hard_error {
            self.closed = true;
            self.pending.clear();
        }
    }

    fn decoded(&self) -> &str {
        &self.decoded
    }
}

impl OpenText {
    pub(super) fn new(max_stop_bytes: usize) -> Self {
        Self {
            text: String::new(),
            streamed_text_len: 0,
            max_stop_bytes,
            stop_searched_len: 0,
            synced_len: 0,
            decoder: GeneratedUtf8::new(),
            #[cfg(test)]
            bytes_copied: 0,
        }
    }

    pub(super) fn text(&self) -> &str {
        &self.text
    }

    pub(super) fn into_text(self) -> String {
        self.text
    }

    pub(super) fn push_piece(&mut self, piece: &[u8], stop_values: &[&str]) -> bool {
        self.sync_decoded(piece);
        let stop_found = stop_in_new_text(
            &self.text,
            self.stop_searched_len,
            stop_values,
            self.max_stop_bytes,
        );
        self.stop_searched_len = self.text.len();
        if stop_found {
            let end = trim_at_stop(&self.text, stop_values).len();
            self.text.truncate(end);
        }
        stop_found
    }

    pub(super) fn plan_emit(&self, flush_all: bool) -> EmitPlan {
        let mut target_len = if flush_all || self.max_stop_bytes == 0 {
            self.text.len()
        } else {
            self.text
                .len()
                .saturating_sub(self.max_stop_bytes.saturating_sub(1))
        };
        while target_len > self.streamed_text_len && !self.text.is_char_boundary(target_len) {
            target_len -= 1;
        }
        if target_len < self.streamed_text_len {
            EmitPlan::Shrink(target_len)
        } else if target_len > self.streamed_text_len {
            EmitPlan::Delta {
                start: self.streamed_text_len,
                end: target_len,
            }
        } else {
            EmitPlan::Idle
        }
    }

    pub(super) fn commit_streamed(&mut self, len: usize) {
        self.streamed_text_len = len;
    }

    fn sync_decoded(&mut self, piece: &[u8]) {
        let before = self.decoder.decoded().len();
        self.decoder.push(piece);
        let decoded_len = self.decoder.decoded().len();
        if self.synced_len == self.text.len() && before == self.text.len() {
            self.append_decoded_suffix(before, decoded_len);
            return;
        }
        if self.decoder.decoded().starts_with(&self.text) {
            self.append_decoded_suffix(self.text.len(), decoded_len);
            return;
        }
        if self.text != self.decoder.decoded() {
            let replacement = self.decoder.decoded().to_owned();
            self.note_copy(replacement.len());
            self.text = replacement;
            self.stop_searched_len = 0;
            self.synced_len = self.text.len();
        }
    }

    fn append_decoded_suffix(&mut self, start: usize, end: usize) {
        if end <= start {
            return;
        }
        let suffix = self.decoder.decoded()[start..end].to_owned();
        self.note_copy(suffix.len());
        self.text.push_str(&suffix);
        self.synced_len = self.text.len();
    }

    fn note_copy(&mut self, bytes: usize) {
        #[cfg(test)]
        {
            self.bytes_copied += bytes;
        }
        #[cfg(not(test))]
        {
            let _ = bytes;
        }
    }

    #[cfg(test)]
    fn bytes_copied(&self) -> usize {
        self.bytes_copied
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Reference {
        bytes: Vec<u8>,
        text: String,
        stop_searched_len: usize,
        streamed_text_len: usize,
        max_stop_bytes: usize,
    }

    impl Reference {
        fn new(max_stop_bytes: usize) -> Self {
            Self {
                bytes: Vec::new(),
                text: String::new(),
                stop_searched_len: 0,
                streamed_text_len: 0,
                max_stop_bytes,
            }
        }

        fn push_piece(&mut self, piece: &[u8], stop_values: &[&str]) -> bool {
            self.bytes.extend_from_slice(piece);
            let valid_len = match std::str::from_utf8(&self.bytes) {
                Ok(_) => self.bytes.len(),
                Err(error) => error.valid_up_to(),
            };
            if valid_len > 0 {
                let candidate =
                    std::str::from_utf8(&self.bytes[..valid_len]).expect("valid prefix");
                if let Some(delta) = candidate.strip_prefix(&self.text) {
                    if !delta.is_empty() {
                        self.text = candidate.to_string();
                    }
                } else if candidate != self.text {
                    self.text = candidate.to_string();
                    self.stop_searched_len = 0;
                }
            }
            let stop_found = stop_in_new_text(
                &self.text,
                self.stop_searched_len,
                stop_values,
                self.max_stop_bytes,
            );
            self.stop_searched_len = self.text.len();
            if stop_found {
                let end = trim_at_stop(&self.text, stop_values).len();
                self.text.truncate(end);
            }
            stop_found
        }

        fn take_delta(&mut self, flush_all: bool) -> Option<String> {
            let mut target_len = if flush_all || self.max_stop_bytes == 0 {
                self.text.len()
            } else {
                self.text
                    .len()
                    .saturating_sub(self.max_stop_bytes.saturating_sub(1))
            };
            while target_len > self.streamed_text_len && !self.text.is_char_boundary(target_len) {
                target_len -= 1;
            }
            if target_len < self.streamed_text_len {
                self.streamed_text_len = target_len;
                None
            } else if target_len > self.streamed_text_len {
                let delta = self.text[self.streamed_text_len..target_len].to_string();
                self.streamed_text_len = target_len;
                Some(delta)
            } else {
                None
            }
        }
    }

    fn drive(pieces: &[&[u8]], stops: &[&str], max_stop_bytes: usize) -> (String, Vec<String>) {
        let mut open = OpenText::new(max_stop_bytes);
        let mut reference = Reference::new(max_stop_bytes);
        let mut emitted = Vec::new();
        for piece in pieces {
            let stopped = open.push_piece(piece, stops);
            let reference_stopped = reference.push_piece(piece, stops);
            assert_eq!(stopped, reference_stopped);
            assert_eq!(open.text(), reference.text);
            assert_eq!(open.stop_searched_len, reference.stop_searched_len);
            let delta = take_delta(&mut open, stopped);
            assert_eq!(delta, reference.take_delta(stopped));
            if let Some(delta) = delta {
                emitted.push(delta);
            }
        }
        if !pieces.is_empty() {
            let delta = take_delta(&mut open, true);
            assert_eq!(delta, reference.take_delta(true));
            if let Some(delta) = delta {
                emitted.push(delta);
            }
        }
        (open.text().to_string(), emitted)
    }

    fn take_delta(open: &mut OpenText, flush_all: bool) -> Option<String> {
        match open.plan_emit(flush_all) {
            EmitPlan::Delta { start, end } => {
                let delta = open.text()[start..end].to_string();
                open.commit_streamed(end);
                Some(delta)
            }
            EmitPlan::Shrink(len) => {
                open.commit_streamed(len);
                None
            }
            EmitPlan::Idle => None,
        }
    }

    #[test]
    fn multibyte_utf8_split_across_tokens_matches_the_full_prefix() {
        let euro = "€".as_bytes();
        let emoji = "😀".as_bytes();
        let mut pieces = Vec::new();
        for byte in euro.iter().chain(emoji).copied() {
            pieces.push(vec![byte]);
        }
        let refs: Vec<&[u8]> = pieces.iter().map(Vec::as_slice).collect();
        let (text, emitted) = drive(&refs, &[], 0);
        assert_eq!(text, "€😀");
        assert_eq!(emitted.concat(), "€😀");
        assert_eq!(emitted, vec!["€".to_string(), "😀".to_string()]);
    }

    #[test]
    fn split_multibyte_rescans_only_the_incomplete_tail() {
        use crate::frontend::util::{reset_utf8_bytes_scanned, utf8_bytes_scanned};

        reset_utf8_bytes_scanned();
        let euro = "€".as_bytes();
        let pieces: Vec<Vec<u8>> = euro.iter().copied().map(|byte| vec![byte]).collect();
        let refs: Vec<&[u8]> = pieces.iter().map(Vec::as_slice).collect();
        let (text, _) = drive(&refs, &[], 0);
        assert_eq!(text, "€");
        // 1 + 2 + 3: each new byte is scanned with the still-incomplete tail,
        // not with the bytes already committed.
        assert_eq!(utf8_bytes_scanned(), 6);
    }

    #[test]
    fn invalid_utf8_does_not_decode_bytes_after_the_error() {
        let (text, _) = drive(&[b"hi", &[0xff], b"there"], &[], 0);
        assert_eq!(text, "hi");
    }

    #[test]
    fn long_stream_examines_bytes_proportional_to_the_new_piece() {
        use crate::frontend::util::{reset_utf8_bytes_scanned, utf8_bytes_scanned};

        reset_utf8_bytes_scanned();
        let mut open = OpenText::new(0);
        let tokens = 4_096;
        for _ in 0..tokens {
            open.push_piece(b"a", &[]);
        }
        assert_eq!(open.text().len(), tokens);
        let scanned = utf8_bytes_scanned();
        assert_eq!(scanned, tokens);
        assert_eq!(open.bytes_copied(), tokens);
    }

    #[test]
    fn stop_that_spans_tokens_trims_and_flush_releases_the_holdback() {
        let (text, emitted) = drive(&[b"hello ST", b"OP tail"], &["STOP"], "STOP".len());
        assert_eq!(text, "hello ");
        assert_eq!(emitted, vec!["hello".to_string(), " ".to_string()]);
    }

    #[test]
    fn final_flush_releases_bytes_held_for_a_stop_sequence() {
        let (text, emitted) = drive(&[b"abcdef"], &[], 4);
        assert_eq!(text, "abcdef");
        assert_eq!(emitted, vec!["abc".to_string(), "def".to_string()]);
    }

    #[test]
    fn final_flush_keeps_an_incomplete_utf8_suffix_out_of_the_text() {
        let (text, emitted) = drive(&[&[b'A', 0xE2, 0x82]], &[], 0);
        assert_eq!(text, "A");
        assert_eq!(emitted.concat(), "A");
    }

    #[test]
    fn holdback_walks_back_to_a_char_boundary() {
        let (text, emitted) = drive(&["aéé".as_bytes()], &[], 3);
        assert_eq!(text, "aéé");
        assert_eq!(emitted, vec!["aé".to_string(), "é".to_string()]);
    }

    #[test]
    fn incomplete_sequence_that_becomes_invalid_drops_the_following_bytes() {
        let (text, emitted) = drive(&[&[0xE2], &[0xFF], b"Z"], &[], 0);
        assert_eq!(text, "");
        assert!(emitted.is_empty());
    }

    #[test]
    fn earliest_stop_wins_when_the_match_spans_tokens() {
        let (text, emitted) = drive(&[b"xxB", b"Ayy"], &["BA", "Ayy"], 3);
        assert_eq!(text, "xx");
        assert_eq!(emitted, vec!["x".to_string(), "x".to_string()]);
    }

    #[test]
    fn bytes_after_a_stop_follow_the_previous_prefix_rules() {
        let (text, emitted) = drive(&[b"hello ST", b"OP", b"X"], &["STOP"], "STOP".len());
        assert_eq!(text, "hello STOPX");
        assert_eq!(
            emitted,
            vec![
                "hello".to_string(),
                " ".to_string(),
                "ST".to_string(),
                "OPX".to_string(),
            ]
        );
    }

    #[test]
    fn chunked_mixed_bytes_match_the_previous_decoder() {
        let mut raw = b"pr".to_vec();
        raw.extend_from_slice("€".as_bytes());
        raw.extend_from_slice(b" END tail");
        raw.push(0xff);
        raw.extend_from_slice(b"NO");
        for width in [1_usize, 2, 3, 5] {
            let pieces: Vec<Vec<u8>> = raw.chunks(width).map(<[u8]>::to_vec).collect();
            let refs: Vec<&[u8]> = pieces.iter().map(Vec::as_slice).collect();
            let (text, emitted) = drive(&refs, &["END"], "END".len());
            assert_eq!(text, "pr€ END tail", "width {width}");
            assert_eq!(emitted.concat(), text, "width {width}");
            assert!(!text.contains("NO"), "width {width}");
        }
    }

    #[test]
    fn rewritten_text_resets_stop_search_before_the_next_check() {
        let mut open = OpenText::new("hello".len());
        open.push_piece(b"hello", &[]);
        open.text = "zzzz".to_string();
        open.synced_len = open.text.len();
        open.stop_searched_len = 100;
        let stopped = open.push_piece(b"!", &["hello"]);
        assert!(stopped);
        assert_eq!(open.text(), "");
    }
}
