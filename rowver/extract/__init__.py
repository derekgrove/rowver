"""CLI extractors: everything the viewer would show, straight from a .rvr."""


class ExtractError(ValueError):
    """The cartridge or the request cannot produce the asked-for extraction."""
