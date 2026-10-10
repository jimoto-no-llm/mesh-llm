use std::time::Instant;

use skippy_runtime::SamplingConfig;

use crate::CacheAffinity;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SequenceStatus {
    Waiting,
    Running,
    Preempted,
    Finished,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixRestoreKind {
    ResidentKv,
    RecurrentWholeState,
    KvAndRecurrentWholeState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefixRestore {
    pub page_id: String,
    pub token_count: usize,
    pub kind: PrefixRestoreKind,
}

#[derive(Debug, Clone)]
pub struct Sequence {
    pub id: String,
    pub prompt_tokens: Vec<i32>,
    pub generated_tokens: Vec<i32>,
    pub max_tokens: u32,
    pub sampling: Option<SamplingConfig>,
    pub priority: u64,
    /// Admission reservation for this request across scheduler memory
    /// components. This is deliberately separate from `max_tokens`: the
    /// client value is a generation ceiling, while serving admission reserves
    /// bounded decode headroom plus the concrete prompt.
    pub admission_tokens: usize,
    pub status: SequenceStatus,
    pub prefix_restore: Option<PrefixRestore>,
    pub cache_affinity: CacheAffinity,
    pub admitted_at: Option<Instant>,
    pub(crate) prefill_cursor: usize,
    pub(crate) enqueued_turn: u64,
    pub(crate) enqueue_order: u64,
}

impl Sequence {
    pub fn new(
        id: String,
        prompt_tokens: Vec<i32>,
        max_tokens: u32,
        sampling: Option<SamplingConfig>,
        priority: u64,
    ) -> Self {
        let admission_tokens = prompt_tokens
            .len()
            .saturating_add(usize::try_from(max_tokens).unwrap_or(usize::MAX));
        Self {
            id,
            prompt_tokens,
            generated_tokens: Vec::new(),
            max_tokens,
            sampling,
            priority,
            admission_tokens,
            status: SequenceStatus::Waiting,
            prefix_restore: None,
            cache_affinity: CacheAffinity::default(),
            admitted_at: None,
            prefill_cursor: 0,
            enqueued_turn: 0,
            enqueue_order: 0,
        }
    }

    pub fn with_admission_tokens(mut self, admission_tokens: usize) -> Self {
        self.admission_tokens = admission_tokens.max(self.prompt_tokens.len());
        self
    }

    pub fn with_prefix_restore(mut self, restore: PrefixRestore) -> Self {
        // A restored prefix does not carry a sampled next token. Keep one
        // replay token runnable so the native runtime produces logits even
        // when the cache covers the complete prompt.
        let replay_len = self.recompute_token_count();
        self.prefill_cursor = restore.token_count.min(replay_len.saturating_sub(1));
        self.prefix_restore = Some(restore);
        self
    }

    pub fn with_cache_affinity(mut self, affinity: CacheAffinity) -> Self {
        self.cache_affinity = affinity;
        self
    }

    /// Resume scheduler ownership after an external cache-aware prefill has
    /// already produced the first unconsumed token for this runtime session.
    pub fn with_prefilled_generation(mut self, generated_tokens: Vec<i32>) -> Self {
        self.generated_tokens = generated_tokens;
        self.prefill_cursor = self.recompute_token_count();
        self
    }

    pub fn is_finished(&self) -> bool {
        matches!(
            self.status,
            SequenceStatus::Finished | SequenceStatus::Failed
        )
    }

    pub fn recompute_tokens(&self) -> Vec<i32> {
        let replay_generated = self.generated_tokens.len().saturating_sub(1);
        let mut tokens = Vec::with_capacity(self.prompt_tokens.len() + replay_generated);
        tokens.extend_from_slice(&self.prompt_tokens);
        tokens.extend_from_slice(&self.generated_tokens[..replay_generated]);
        tokens
    }

    /// Copy only `[start, end)` of the recompute stream without materializing
    /// the full prompt + generated prefix. The hot iteration loop schedules
    /// small prefill chunks out of potentially very long contexts, so building
    /// the whole replay vector per sequence per iteration was O(context) alloc
    /// + memcpy for O(chunk) useful bytes.
    pub(crate) fn copy_recompute_range(&self, start: usize, end: usize) -> Vec<i32> {
        let end = end.min(self.recompute_token_count());
        let start = start.min(end);
        let prompt_len = self.prompt_tokens.len();
        let mut out = Vec::with_capacity(end - start);
        if start < prompt_len {
            let prompt_end = end.min(prompt_len);
            out.extend_from_slice(&self.prompt_tokens[start..prompt_end]);
        }
        if end > prompt_len {
            let gen_start = start.saturating_sub(prompt_len);
            let gen_end = end - prompt_len;
            // Generated tail excludes the pending decode token, matching
            // `recompute_tokens`.
            let replay_generated = self.generated_tokens.len().saturating_sub(1);
            let gen_end = gen_end.min(replay_generated);
            if gen_start < gen_end {
                out.extend_from_slice(&self.generated_tokens[gen_start..gen_end]);
            }
        }
        out
    }

    pub(crate) fn recompute_token_count(&self) -> usize {
        self.prompt_tokens
            .len()
            .saturating_add(self.generated_tokens.len().saturating_sub(1))
    }

    pub(crate) fn pending_decode_token(&self) -> Option<i32> {
        self.generated_tokens.last().copied()
    }

    pub(crate) fn reset_for_recompute(&mut self) {
        self.status = SequenceStatus::Preempted;
        self.admitted_at = None;
        self.prefill_cursor = self
            .prefix_restore
            .as_ref()
            .map_or(0, |restore| restore.token_count)
            .min(self.recompute_token_count());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IterationPhase {
    Prefill,
    Recompute,
    Decode,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IterationWork {
    pub sequence_id: String,
    pub tokens: Vec<i32>,
    pub positions: Vec<i32>,
    pub sample_last: bool,
    pub phase: IterationPhase,
    pub sampling: Option<SamplingConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IterationPrediction {
    pub work_index: usize,
    pub token: i32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct IterationPlan {
    pub work: Vec<IterationWork>,
    pub token_count: usize,
    pub admitted: usize,
    pub preempted: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sequence_with_history(prompt_len: usize, generated: Vec<i32>) -> Sequence {
        let mut sequence = Sequence::new(
            "test".to_string(),
            (0..prompt_len as i32).collect(),
            16,
            None,
            1,
        );
        sequence.generated_tokens = generated;
        sequence
    }

    #[test]
    fn copy_recompute_range_matches_full_replay_slices() {
        let sequence = sequence_with_history(8, vec![100, 101, 102, 103]);
        // Replay is prompt[0..8] + generated[0..3]; pending 103 is excluded.
        let replay = sequence.recompute_tokens();
        assert_eq!(replay.len(), sequence.recompute_token_count());
        for start in 0..=replay.len() {
            for end in start..=replay.len() {
                assert_eq!(
                    sequence.copy_recompute_range(start, end),
                    replay[start..end],
                    "mismatch for [{start}, {end})"
                );
            }
        }
    }

    #[test]
    fn copy_recompute_range_handles_prompt_only_and_empty_generated() {
        let prompt_only = sequence_with_history(5, Vec::new());
        assert_eq!(prompt_only.copy_recompute_range(1, 4), vec![1, 2, 3]);
        // Clamps out-of-range ends instead of panicking on hot scheduling paths.
        assert_eq!(prompt_only.copy_recompute_range(3, 99).len(), 2);
    }
}
