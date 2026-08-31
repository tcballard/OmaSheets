from pathlib import Path
import unittest

from omasheets.errors import PolicyError
from omasheets.policy import (
    Actor,
    PublishMode,
    WorkbookFormat,
    conversion_destination,
    require_agent_readable,
    require_publish_authority,
    require_stageable,
    workbook_format,
)


class PolicyTests(unittest.TestCase):
    def test_supported_formats_are_case_insensitive(self) -> None:
        self.assertIs(workbook_format(Path("book.XLS")), WorkbookFormat.XLS)
        self.assertIs(workbook_format(Path("book.xlsx")), WorkbookFormat.XLSX)

    def test_unknown_format_is_rejected(self) -> None:
        with self.assertRaises(PolicyError):
            workbook_format(Path("book.csv"))

    def test_agents_can_read_all_supported_formats(self) -> None:
        for suffix in ("xls", "xlsx", "xlsm", "ods"):
            require_agent_readable(Path(f"book.{suffix}"))

    def test_agents_cannot_stage_legacy_or_macro_workbooks(self) -> None:
        for suffix in ("xls", "xlsm"):
            with self.assertRaises(PolicyError):
                require_stageable(Path(f"book.{suffix}"), actor=Actor.AGENT)

    def test_stageable_formats(self) -> None:
        for suffix in ("xlsx", "ods"):
            require_stageable(Path(f"book.{suffix}"), actor=Actor.AGENT)

    def test_agents_never_publish(self) -> None:
        for mode in PublishMode:
            with self.assertRaises(PolicyError):
                require_publish_authority(actor=Actor.AGENT, mode=mode)

    def test_legacy_conversion_is_adjacent_xlsx(self) -> None:
        self.assertEqual(
            conversion_destination(Path("/tmp/legacy.xls")),
            Path("/tmp/legacy.xlsx"),
        )


if __name__ == "__main__":
    unittest.main()

