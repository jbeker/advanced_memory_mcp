import json
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class TokenEntry:
    file: str
    mode: str

    def __post_init__(self):
        if self.mode not in ("read-only", "read-write"):
            raise ValueError(f"Invalid mode '{self.mode}': must be 'read-only' or 'read-write'")
        if not self.file:
            raise ValueError("Token entry 'file' must not be empty")


class TokenConfig:
    """Loads a tokens.json config and provides lookup by bearer token."""

    def __init__(self, config_path: str):
        path = Path(config_path)
        if not path.exists():
            raise FileNotFoundError(f"Token config file not found: {config_path}")

        with open(path) as f:
            raw = json.load(f)

        if not isinstance(raw, dict) or not raw:
            raise ValueError("Token config must be a non-empty JSON object")

        self._entries: dict[str, TokenEntry] = {}
        for token, entry in raw.items():
            if not isinstance(entry, dict):
                raise ValueError(f"Token entry for '{token}' must be a JSON object")
            self._entries[token] = TokenEntry(
                file=entry.get("file", ""),
                mode=entry.get("mode", "read-only"),
            )

    def get(self, token: str) -> TokenEntry:
        """Look up a token. Raises ValueError if the token is unknown."""
        entry = self._entries.get(token)
        if entry is None:
            raise ValueError("Unknown or invalid authentication token.")
        return entry
