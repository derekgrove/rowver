"""What a recipe imports: `from rowver import Cart`."""

__version__ = "11.1.0"


class Cart:
    """What a recipe exports: the fileset, the hooks, the columns, and labels."""

    def __init__(self, *, fileset, columns, keep, prepare=None, labels=None, title=None, note=None):
        self.fileset, self.columns, self.keep, self.prepare = fileset, columns, keep, prepare
        self.labels, self.title, self.note = labels or {}, title, note
