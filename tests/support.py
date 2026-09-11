"""A complete recipe snapshot for cartridges the tests pack by hand."""
from rowver import rvr

KEEP = {"kind": "python", "name": "keep", "doc": "",
        "source": "def keep(events):\n    return events.pt > 0"}


def snapshot(*samples: str, **overrides) -> dict:
    """A valid ``made`` drawer naming ``samples`` with no files; pack digests it."""
    return {"study_name": "study", "by": f"RowVer {rvr.VERSION}", "built": "test",
            "samples": {name: {"files": {}, "weights": None, "metadata": {}} for name in samples},
            "keep": KEEP, "prepare": None, "cart": None, **overrides}
