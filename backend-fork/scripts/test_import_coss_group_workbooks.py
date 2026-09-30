import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from import_coss_group_workbooks import employee_identity_source_key


class EmployeeIdentitySourceKeyTests(unittest.TestCase):
    def test_low_confidence_same_name_same_site_same_hire_date_keeps_source_rows_separate(self):
        first_key, first_meta = employee_identity_source_key(
            org_slug="coss",
            name="김동명이인",
            employee_number=None,
            legal_identifier=None,
            birth=None,
            hire_date="2026-01-01",
            worksite_name="본사",
            source_filename="a.xlsx",
            source_sheet="명부",
            source_row=10,
        )
        second_key, second_meta = employee_identity_source_key(
            org_slug="coss",
            name="김동명이인",
            employee_number=None,
            legal_identifier=None,
            birth=None,
            hire_date="2026-01-01",
            worksite_name="본사",
            source_filename="a.xlsx",
            source_sheet="명부",
            source_row=11,
        )

        self.assertNotEqual(first_key, second_key)
        self.assertEqual(first_meta["strategy"], "source_row_fingerprint")
        self.assertEqual(second_meta["strategy"], "source_row_fingerprint")
        self.assertTrue(first_meta["manual_review_required"])
        self.assertFalse(first_meta["name_only_merge"])

    def test_employee_number_is_high_confidence_and_dedupes_across_source_rows(self):
        first_key, first_meta = employee_identity_source_key(
            org_slug="knl",
            name="김사번",
            employee_number="A-001",
            legal_identifier=None,
            birth=None,
            hire_date="2026-01-01",
            worksite_name="본사",
            source_filename="a.xlsx",
            source_sheet="명부",
            source_row=10,
        )
        second_key, second_meta = employee_identity_source_key(
            org_slug="knl",
            name="김사번",
            employee_number="A-001",
            legal_identifier=None,
            birth=None,
            hire_date="2026-01-01",
            worksite_name="본사",
            source_filename="b.xlsx",
            source_sheet="명부",
            source_row=99,
        )

        self.assertEqual(first_key, second_key)
        self.assertEqual(first_meta["strategy"], "employee_number")
        self.assertEqual(second_meta["confidence"], "high")
        self.assertFalse(first_meta["manual_review_required"])

    def test_legal_identifier_key_is_hashed_and_never_contains_raw_digits(self):
        key, meta = employee_identity_source_key(
            org_slug="coss",
            name="김식별",
            employee_number=None,
            legal_identifier="TEST-ID-1234-5678",
            birth=None,
            hire_date=None,
            worksite_name=None,
            source_filename="a.xlsx",
            source_sheet="명부",
            source_row=10,
        )

        self.assertEqual(meta["strategy"], "legal_identifier_hash")
        self.assertEqual(meta["confidence"], "high")
        self.assertNotIn("1234", key)
        self.assertNotIn("5678", key)


if __name__ == "__main__":
    unittest.main()
