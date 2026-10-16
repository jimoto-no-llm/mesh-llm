"""Simulation of the community credit ledger proposed in RFC #45.

This is an evaluation artifact, not production code. It answers the RFC's
completion conditions: a contributor earns credit for verified completed work
and can spend it on inference; self-loops, failed jobs, and duplicate reports
earn nothing; issuance and consumption can be recomputed from the entry log;
no prompt text and no personal data are stored.

The ledger is deterministic: the caller supplies the epoch, so a run is
replayable without a clock.

    python3 mesh/evals/credit_ledger_simulation.py
"""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum


class JobStatus(str, Enum):
    COMPLETED = "completed"
    FAILED = "failed"


class StageRole(str, Enum):
    FIRST = "first"
    MIDDLE = "middle"
    LAST = "last"
    WHOLE = "whole"


class EntryKind(str, Enum):
    ISSUE = "issue"
    CONSUME = "consume"
    EXPIRE = "expire"


MODEL_WEIGHTS: dict[str, float] = {
    "small-3b": 1.0,
    "medium-8b": 2.0,
    "large-70b": 8.0,
}

STAGE_WEIGHTS: dict[StageRole, float] = {
    StageRole.FIRST: 1.2,
    StageRole.MIDDLE: 1.0,
    StageRole.LAST: 1.2,
    StageRole.WHOLE: 1.0,
}

DEFAULT_CREDIT_TTL_EPOCHS = 30


@dataclass(frozen=True)
class Contribution:
    """One reported unit of work. Identifiers are opaque; no content is stored."""

    job_id: str
    node: str
    requester: str
    tokens: int
    model_class: str
    stage_role: StageRole
    status: JobStatus
    verified: bool
    epoch: int


@dataclass(frozen=True)
class LedgerEntry:
    entry_id: int
    kind: EntryKind
    node: str
    amount: float
    epoch: int
    reason: str
    job_id: str | None = None


@dataclass
class CreditLedger:
    """Append-only credit ledger; balances are recomputable from the entries."""

    credit_ttl_epochs: int = DEFAULT_CREDIT_TTL_EPOCHS
    entries: list[LedgerEntry] = field(default_factory=list)
    rejected: list[tuple[str, str]] = field(default_factory=list)
    _seen_jobs: set[str] = field(default_factory=set)
    _expired_entries: set[int] = field(default_factory=set)
    _next_entry_id: int = 1

    def earn(self, contribution: Contribution) -> LedgerEntry | None:
        """Record a contribution, or reject it entirely with a reason."""
        if contribution.job_id in self._seen_jobs:
            self.rejected.append((contribution.job_id, "duplicate_job"))
            return None
        if contribution.status is not JobStatus.COMPLETED:
            self.rejected.append((contribution.job_id, "job_not_completed"))
            return None
        if not contribution.verified:
            self.rejected.append((contribution.job_id, "unverified_work"))
            return None
        if contribution.node == contribution.requester:
            self.rejected.append((contribution.job_id, "self_loop"))
            return None
        if contribution.tokens <= 0:
            self.rejected.append((contribution.job_id, "no_work"))
            return None
        weight = MODEL_WEIGHTS.get(contribution.model_class)
        if weight is None:
            self.rejected.append((contribution.job_id, "unknown_model_class"))
            return None

        amount = contribution.tokens * weight * STAGE_WEIGHTS[contribution.stage_role]
        self._seen_jobs.add(contribution.job_id)
        return self._append(
            EntryKind.ISSUE,
            contribution.node,
            amount,
            contribution.epoch,
            "verified_contribution",
            contribution.job_id,
        )

    def spend(self, node: str, amount: float, epoch: int, job_id: str) -> LedgerEntry | None:
        """Charge amount for inference. Never allows a negative balance."""
        if amount <= 0:
            self.rejected.append((job_id, "no_charge"))
            return None
        if self.balance(node) < amount:
            self.rejected.append((job_id, "insufficient_credit"))
            return None
        return self._append(EntryKind.CONSUME, node, -amount, epoch, "inference", job_id)

    def expire(self, node: str, epoch: int) -> float:
        """Expire issued credit older than the TTL. Returns the amount expired."""
        expired = 0.0
        for entry in list(self.entries):
            if entry.node != node or entry.kind is not EntryKind.ISSUE:
                continue
            if entry.reason != "verified_contribution":
                continue
            if entry.entry_id in self._expired_entries:
                continue
            if epoch - entry.epoch < self.credit_ttl_epochs:
                continue
            # Record the expiry as its own entry and leave the original in place
            # so the audit trail keeps both the issuance and its expiry. The id
            # set is what makes a later sweep idempotent.
            self._expired_entries.add(entry.entry_id)
            expired += entry.amount
            self._append(EntryKind.EXPIRE, node, -entry.amount, epoch, "ttl", entry.job_id)
        return expired

    def balance(self, node: str) -> float:
        return sum(entry.amount for entry in self.entries if entry.node == node)

    def recomputed_balance(self, node: str) -> float:
        """Balance derived only from the recorded entries, for audit."""
        return sum(entry.amount for entry in self.entries if entry.node == node)

    def audit(self) -> dict[str, float]:
        totals: dict[str, float] = {}
        for entry in self.entries:
            totals[entry.node] = totals.get(entry.node, 0.0) + entry.amount
        return totals

    def _append(
        self,
        kind: EntryKind,
        node: str,
        amount: float,
        epoch: int,
        reason: str,
        job_id: str | None,
    ) -> LedgerEntry:
        entry = LedgerEntry(self._next_entry_id, kind, node, amount, epoch, reason, job_id)
        self._next_entry_id += 1
        self.entries.append(entry)
        return entry


def contribution(job_id: str, tokens: int = 1000, epoch: int = 1, **overrides: object) -> Contribution:
    """Build a contribution with simulation defaults."""
    base: dict[str, object] = {
        "job_id": job_id,
        "node": "node-a",
        "requester": "node-b",
        "tokens": tokens,
        "model_class": "medium-8b",
        "stage_role": StageRole.FIRST,
        "status": JobStatus.COMPLETED,
        "verified": True,
        "epoch": epoch,
    }
    base.update(overrides)
    return Contribution(**base)  # type: ignore[arg-type]


def run_simulation() -> dict[str, object]:
    """Contribute, abuse, and spend; return the observed totals."""
    ledger = CreditLedger()
    ledger.earn(contribution("job-1"))
    ledger.earn(contribution("job-2", status=JobStatus.FAILED))
    ledger.earn(contribution("loop-1", tokens=10_000, requester="node-a"))
    ledger.earn(contribution("job-1", epoch=2))
    earned = ledger.balance("node-a")
    ledger.spend("node-a", earned / 2, epoch=2, job_id="use-1")
    return {
        "earned": earned,
        "balance": ledger.balance("node-a"),
        "rejected": list(ledger.rejected),
        "audit": ledger.audit(),
        "entries": len(ledger.entries),
    }


if __name__ == "__main__":
    import json

    print(json.dumps(run_simulation(), indent=2, ensure_ascii=False))
