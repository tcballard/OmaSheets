"""Bounded agent intent and explanation metadata for sealed workbook plans."""

from __future__ import annotations

from typing import Any

from .errors import PolicyError

MAX_ASSUMPTIONS = 20
MAX_GROUPS = 20
MAX_EVIDENCE = 50


def _text(value: Any, *, field: str, maximum: int) -> str:
    if not isinstance(value, str) or not value.strip() or len(value) > maximum:
        raise PolicyError(f"workflow {field} must be between 1 and {maximum} characters")
    if any(ord(character) < 32 and character not in "\n\t" for character in value):
        raise PolicyError(f"workflow {field} contains control characters")
    return value.strip()


def validate_workflow(workflow: Any, operation_count: int) -> dict[str, Any]:
    """Validate the explanation shown beside a verified semantic diff."""

    if not isinstance(workflow, dict):
        raise PolicyError("an agent plan requires workflow context")
    allowed = {"goal", "summary", "assumptions", "evidence_ids", "groups"}
    if set(workflow) - allowed or not {"goal", "summary", "evidence_ids", "groups"} <= set(workflow):
        raise PolicyError("workflow has invalid fields")

    assumptions = workflow.get("assumptions", [])
    if not isinstance(assumptions, list) or len(assumptions) > MAX_ASSUMPTIONS:
        raise PolicyError(f"workflow assumptions must contain at most {MAX_ASSUMPTIONS} items")
    normalized_assumptions = [
        _text(item, field=f"assumption {index}", maximum=500)
        for index, item in enumerate(assumptions)
    ]

    evidence_ids = workflow["evidence_ids"]
    if not isinstance(evidence_ids, list) or not 1 <= len(evidence_ids) <= MAX_EVIDENCE:
        raise PolicyError(f"workflow must cite between 1 and {MAX_EVIDENCE} observations")
    if len(set(evidence_ids)) != len(evidence_ids) or any(
        not isinstance(item, str) or len(item) != 32 or any(character not in "0123456789abcdef" for character in item)
        for item in evidence_ids
    ):
        raise PolicyError("workflow contains invalid or duplicate evidence identifiers")

    groups = workflow["groups"]
    if not isinstance(groups, list) or not 1 <= len(groups) <= min(MAX_GROUPS, operation_count):
        raise PolicyError("workflow must group every operation by purpose")
    normalized_groups = []
    covered: list[int] = []
    for index, group in enumerate(groups):
        if not isinstance(group, dict) or set(group) != {"title", "purpose", "operation_indexes"}:
            raise PolicyError(f"workflow group {index} has invalid fields")
        indexes = group["operation_indexes"]
        if not isinstance(indexes, list) or not indexes or any(
            not isinstance(item, int) or isinstance(item, bool) or not 0 <= item < operation_count
            for item in indexes
        ):
            raise PolicyError(f"workflow group {index} has invalid operation indexes")
        covered.extend(indexes)
        normalized_groups.append({
            "title": _text(group["title"], field=f"group {index} title", maximum=120),
            "purpose": _text(group["purpose"], field=f"group {index} purpose", maximum=500),
            "operation_indexes": list(indexes),
        })
    if sorted(covered) != list(range(operation_count)):
        raise PolicyError("workflow groups must cover every operation exactly once")

    return {
        "goal": _text(workflow["goal"], field="goal", maximum=1000),
        "summary": _text(workflow["summary"], field="summary", maximum=2000),
        "assumptions": normalized_assumptions,
        "evidence_ids": list(evidence_ids),
        "groups": normalized_groups,
    }
