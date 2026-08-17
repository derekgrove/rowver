""" TEMPLATE —

    This file configures how the page opens (what plots, tables, cuts the .rvr opens to).
    THIS FILE IS COMPLETELY OPTIONAL, my recommendation is you delete it and do a first
    time setup in the browser and then export that .rvr file as your new starting point.


    What this file is actually useful for:
    When setting up another .rvr file, you can extract a configuration from an old
    .rvr file and just paste it in your new study to automatically get the same layout.
    This of course assumes your new .rvr will have the same cuts (branches) as the previous,

    To extract this file from a .rvr file, you may use the command `extract` from
    `page.py`, also in this project. It reads a .rvr and prints a ready-made view.py,
    so you don't have to write one by hand:

    python page.py extract my_old.rvr > studies/mine/view.py
    python run_rowver.py mine --no-skim

    It lifts the WHOLE file — every tab, with its cuts, plots, boards, grouping and
    colours — so what comes back opens exactly as the .rvr did.

    What comes out is VIEW plus GROUPS, SUBGROUPS, COLOUR and LOCKS where the file has
    them, with a docstring saying which file it came from.

    Also be mindful if you're going to reuse a view from one .rvr for another, if
    you have changed any branch names or cuts between the two you will have to rename
    them accordingly.
"""

#: page-global: samples drawn as one curve
GROUPS = {
    "signal": ["some_signal_sample"],
    "background": ["some_background_sample"],
}

#: one entry per tab. Only "name" is required; everything else is optional.
VIEW = [
    {"name": "overview",
     "cuts": {"PT_jet_lead": [30, 1000], "nJet": [1, 2, 3]},
     "fom": ("signal", "background"),
     "lumi": 400,
     "cms": "Work in progress",
     "plots": [{"mode": "1d", "varx": "MET", "nb": 40, "logy": True}]},

    {"name": "signal vs background",
     "plots": [{"mode": "2d", "varx": "MET", "vary": "PT_jet_lead", "nb": 30,
                "layer": "signal", "layer2": "background", "view": "two"}]},
]
