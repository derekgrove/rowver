"""How the mixed cart opens: data in one group, weighted MC in another, at 10 fb^-1."""

GROUPS = {"data": ["Data 2012 DoubleMuParked"], "MC": ["MC stand-in x2"]}

VIEW = [{"name": "mixed",
         "cuts": {},
         "plots": [{"mode": "1d", "varx": "PT_mu_lead", "nb": 40}],
         "lumi": 10,
         "wt": "w"}]
