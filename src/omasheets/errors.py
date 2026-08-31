"""Typed public errors for policy and state failures."""


class OmaSheetsError(Exception):
    """Base class for expected OmaSheets failures."""


class PolicyError(OmaSheetsError):
    """The requested action is outside the format or authority policy."""


class ConflictError(OmaSheetsError):
    """The selected workbook or plan no longer matches its sealed state."""


class EngineError(OmaSheetsError):
    """The isolated Calc engine could not complete a requested operation."""
