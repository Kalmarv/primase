# FromSoft environment selection evidence

## Local fog volumes

`FogVolume ParamEditor` is not an alias of `FogParamEditor`. The recorded
`scratch/primase-reset/tests-a0b6470ed/draw_param_census.log` shows m10_00 sets
100/101 and m12_02 sets 77–94 with no nonnegative part fog selector for them.
The local SoulsFormats MSBS/MSBE region and event layouts provide no established
replacement selector. Smithbox's Fog Volume annotations describe a local volume
injected into volumetric fog, with its own draw-area, density, noise and light controls.
These descriptions establish neither placement ownership nor the activation join.

The reader retains every set and every stored time sample in `fromsoft.draw_param`
with `SetChosen::Unresolved`. `DrawParam::group` excludes those records from the active
scene environment. Matching a part's numeric fog id, taking id 0, or finding only one
set does not establish the missing join. The old part-slot alias has been removed.
The notice distinguishes retained-but-unselected values from unread fields.

Still required: an observation of a placed volume's identity, its selected GPARAM
group/value id, active file/layer and owning render instance, or equivalent native
consumer evidence. No runtime observation or game launch was performed. Source data
remains available for that comparison; no distance fog is invented from these fields.

## Static hour preview (F2, data layer)

| Source | Existing evidence | Hour path / remaining work |
|---|---|---|
| Sekiro | GPARAM v5 stores an id and time per value. The recorded m11_01_0000 report counts 200 further values for chosen ids; this counts variants, not distinct hours. | The read keeps every stored sample of the selected ids, in stored order. `sample-fromsoft-draw-params{hour}` (a pass, the program's choice of hour) samples them before environment lowering and counts the linear interpolation by name; without it `lower-fromsoft-environment` takes each field's first sample and counts that. |
| Elden Ring | Same GPARAM v5 representation; m10_00_0000 counts 126 further chosen-id values. Weather/event files are separate layers, reported unread by the area environment reader. | Base-file hour sampling is implemented; it does not choose weather, event state or layer precedence. |
| Riders Republic | `AnvilSkyControllers.time_of_day` retains source path keys and ticks. `lower-anvil-sky` already has an explicit hour and samples controller ticks at hour × 120 plus authored scattering tables. | Unify the program's hour control with this existing pass; retain weather as a separate choice. Automatic sun direction and exposure still need the named CPU/global producers in `anvil-content-preview.md`; changing hour cannot invent them. |

The new `GPARAM_HOUR_FILE` and `GPARAM_HOUR` census lines read the original fields before
first-value selection, recording each multi-sample id's exact times and values. `GPARAM_LAYER` tests known weather/event layer path
candidates. The existing four-map, first-eight-base-files bound is explicit; this is not an
all-map census, and no new measurement is claimed until it runs. Existing counts above are
from `tests-a0b6470ed/draw_param_census.log`, not estimates of complete coverage.

`hour` is optional, finite and in 0..=24; 24 is midnight. The lab exposes a fractional numeric
control with unset as its stated default, and records the effective value in the read options.
It requires environment reading. Selected floating scalars/vectors interpolate linearly in
stored units over midnight; integers, booleans, text and byte colours hold the preceding key.
Interpolation, including raw stored angles, is a reported preview policy, not proof of the
engine's interpolation curve. Conflicting equal-hour keys and missing/invalid varying times
refuse the environment by name. No other value id is mixed in, and unresolved FogVolume
sets stay unselected and retain all their samples.

Next for Elden weather: use the layer census to establish present files and track domains,
then identify the active weather id, event overrides and blend/precedence rule. Retain layers
independently before applying a proven ordering. A weather file's presence alone cannot decide
which layer wins. A moving Timeline/day-night UI can later re-read with recorded hour choices;
this change implements static data evaluation only.
