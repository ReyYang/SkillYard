from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any


@dataclass(frozen=True)
class Operation:
    action: str
    target: str
    detail: dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True)
class Plan:
    title: str
    operations: list[Operation]
    confirmation: str
    payload: dict[str, Any] = field(default_factory=dict)

    def as_dict(self) -> dict[str, Any]:
        return {
            "title": self.title,
            "confirmation": self.confirmation,
            "operations": [
                {"action": op.action, "target": op.target, "detail": op.detail}
                for op in self.operations
            ],
            "payload": self.payload,
        }
