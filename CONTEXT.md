# RowVer

RowVer is the domain of preparing compact high-energy-physics datasets for
interactive exploration and sharing the resulting analysis view as a file.

## Language

**Study**:
An analysis recipe that identifies source samples, selects events, and names the
data available for exploration.
_Avoid_: Config, job

**Cartridge**:
A self-contained `.rvr` file containing a study's selected events and columns
together with a saved interactive view.
_Avoid_: Skim, project file

**Presser**:
The component that turns a study into a cartridge. This repository is the
presser only; the viewer and verification tooling live in their own repositories.
_Avoid_: Converter, builder

**Viewer**:
The browser application in which a cartridge is explored and its view is edited.
_Avoid_: Frontend, dashboard

**Sample**:
A named collection of collision data or simulated events from one or more source
files. Collision data is unweighted; simulated events may be weighted or
unweighted according to the study's needs.
_Avoid_: Dataset

**Column**:
A named numeric or boolean value retained for every selected event and available to the
viewer for plotting and selections.
_Avoid_: Variable, branch, field

**Region map**:
A partition of selected events into named analysis regions.
_Avoid_: Binning, category tree

**View**:
The plots, tabs, selections, region maps, sample presentation, and visual theme
saved in a cartridge.
_Avoid_: Session, workspace

**Independent verification**:
Evidence that an observable is correct because it was reproduced without sharing
the logic that produced the observable.
_Avoid_: Regression test, cross-check test
