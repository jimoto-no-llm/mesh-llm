"""Evaluation cases for the community credit ledger simulation (RFC #45)."""

from __future__ import annotations

import unittest

from mesh.evals.credit_ledger_simulation import (
    CreditLedger,
    EntryKind,
    JobStatus,
    StageRole,
    contribution,
    run_simulation,
)


class CreditIssuanceTest(unittest.TestCase):
    def test_verified_contribution_earns_credit_that_can_be_spent(self) -> None:
        ledger = CreditLedger()
        entry = ledger.earn(contribution("job-1", tokens=1000, model_class="medium-8b"))
        self.assertIsNotNone(entry)
        # 1000 tokens x 2.0 (medium) x 1.2 (first stage)
        self.assertAlmostEqual(ledger.balance("node-a"), 2400.0)
        spent = ledger.spend("node-a", 1200.0, epoch=2, job_id="use-1")
        self.assertIsNotNone(spent)
        self.assertAlmostEqual(ledger.balance("node-a"), 1200.0)

    def test_middle_stage_earns_the_base_rate(self) -> None:
        ledger = CreditLedger()
        ledger.earn(contribution("job-1", tokens=1000, stage_role=StageRole.MIDDLE))
        self.assertAlmostEqual(ledger.balance("node-a"), 2000.0)

    def test_unknown_model_class_is_rejected_rather_than_priced_by_guess(self) -> None:
        ledger = CreditLedger()
        self.assertIsNone(ledger.earn(contribution("job-1", model_class="mystery")))
        self.assertEqual(ledger.balance("node-a"), 0.0)


class AbuseCaseTest(unittest.TestCase):
    def test_failed_job_earns_nothing(self) -> None:
        ledger = CreditLedger()
        self.assertIsNone(ledger.earn(contribution("job-1", status=JobStatus.FAILED)))
        self.assertEqual(ledger.balance("node-a"), 0.0)

    def test_self_loop_earns_nothing(self) -> None:
        ledger = CreditLedger()
        self.assertIsNone(ledger.earn(contribution("loop-1", requester="node-a")))
        self.assertEqual(ledger.balance("node-a"), 0.0)

    def test_unverified_work_earns_nothing(self) -> None:
        ledger = CreditLedger()
        self.assertIsNone(ledger.earn(contribution("job-1", verified=False)))
        self.assertEqual(ledger.balance("node-a"), 0.0)

    def test_duplicate_job_report_is_paid_once(self) -> None:
        ledger = CreditLedger()
        ledger.earn(contribution("job-1"))
        first = ledger.balance("node-a")
        self.assertIsNone(ledger.earn(contribution("job-1", epoch=2)))
        self.assertAlmostEqual(ledger.balance("node-a"), first)
        self.assertIn(("job-1", "duplicate_job"), ledger.rejected)

    def test_overspend_is_refused_and_never_goes_negative(self) -> None:
        ledger = CreditLedger()
        ledger.earn(contribution("job-1"))
        self.assertIsNone(ledger.spend("node-a", 99_999.0, epoch=2, job_id="use-1"))
        self.assertAlmostEqual(ledger.balance("node-a"), 2400.0)
        self.assertIn(("use-1", "insufficient_credit"), ledger.rejected)


class LedgerAuditTest(unittest.TestCase):
    def test_balance_is_recomputable_from_the_entry_log(self) -> None:
        ledger = CreditLedger()
        ledger.earn(contribution("job-1"))
        ledger.earn(contribution("job-2", node="node-c"))
        ledger.spend("node-a", 400.0, epoch=2, job_id="use-1")
        self.assertAlmostEqual(ledger.balance("node-a"), ledger.recomputed_balance("node-a"))
        self.assertEqual(ledger.audit(), {"node-a": 2000.0, "node-c": 2400.0})

    def test_expiry_removes_unused_credit_once(self) -> None:
        ledger = CreditLedger(credit_ttl_epochs=10)
        ledger.earn(contribution("job-1", epoch=1))
        self.assertEqual(ledger.expire("node-a", epoch=5), 0.0)
        self.assertAlmostEqual(ledger.expire("node-a", epoch=11), 2400.0)
        self.assertAlmostEqual(ledger.balance("node-a"), 0.0)
        # A second sweep must not double-expire.
        self.assertEqual(ledger.expire("node-a", epoch=40), 0.0)
        self.assertAlmostEqual(ledger.balance("node-a"), 0.0)

    def test_entries_carry_no_prompt_or_content_fields(self) -> None:
        ledger = CreditLedger()
        entry = ledger.earn(contribution("job-1"))
        assert entry is not None
        self.assertEqual(
            set(vars(entry)),
            {"entry_id", "kind", "node", "amount", "epoch", "reason", "job_id"},
        )

    def test_simulation_reports_the_expected_shape(self) -> None:
        result = run_simulation()
        self.assertAlmostEqual(result["earned"], 2400.0)
        self.assertAlmostEqual(result["balance"], 1200.0)
        reasons = [reason for _, reason in result["rejected"]]
        self.assertEqual(reasons, ["job_not_completed", "self_loop", "duplicate_job"])
        self.assertEqual(result["entries"], 2)


if __name__ == "__main__":
    unittest.main()
