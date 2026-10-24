# Controls

**Azeroth Survivors** — top-down, one hand on WASD, the other on the wheel.

| When | Key / Input | Action |
|---|---|---|
| Menu | Click a race, gender, class and zone | make your pick (**Randomize** rolls one) |
| Menu | **Enter** (or **Enter World**) | start the run; the world travels to the zone picked |
| Playing | **W / S / A / D** or **arrow keys** | move the hero (locked camera: the hero faces the cursor, backpedals and strafes) |
| Playing | **Space** | jump |
| Playing | **F** (from level 5) | your class's movement spell: Blink, Charge… aim at the cursor |
| Playing | **C** | unlock / lock the camera |
| Any | **G** | toggle the picture polish (`?grade=0` starts it off) |
| Playing | **mouse wheel** | zoom, from over the shoulder out to 70 yd |
| Playing | **drag** (camera unlocked) | swing and tilt the camera |
| Playing | **Esc** | pause |
| Playing | — | abilities cast themselves (no aim) |
| Level up | **1 / 2 / 3** (or click a card) | pick an upgrade |
| Paused | **Esc** | resume |
| Game over | **Enter** (or **Play again**) | back to the menu |
| Any | minimap **+ / −** / minimize | minimap zoom / collapse |

The camera follows the hero, locked top-down by default (laptop-friendly). **C** unlocks it so a
mouse drag swings and tilts it; **C** again snaps it back. In the menu it slowly orbits the
preview hero. Your race, gender, class and zone are remembered across reloads.

## Gamepad

| When | Input | Action |
|---|---|---|
| Playing | **left stick / D-pad** | move; the hero faces where he runs |
| Playing | **right stick** (camera locked) | aim: the hero faces it while held (twin-stick) |
| Playing | **right stick** (camera unlocked) | swing and tilt the camera; movement follows the camera |
| Menu / level up / paused / game over | **D-pad / left stick** | jump to the next button that way (held: repeat) |
| Menu / level up / paused / game over | **right stick** | move the cursor freely |
| Menu / level up / paused / game over | **A / east** | click the highlighted button or card |
| Playing | **B / south** | jump |
| Playing | **A / east** | class movement spell (from level 5): Blink goes where you face, Charge takes the enemy ahead |
| Playing | **X / north** | unlock / lock the camera |
| Playing | **L / R** | zoom out / in |
| Menu | **+ / Start** | Enter World |
| Playing / paused | **+ / Start** | pause / resume |
| Paused / game over | **− / Select** | quit the game |

The camera is the same with a pad as with the mouse: locked, it sits at a fixed 61° tilt, and the
zoom is by wheel or **L / R**. While the right stick steers, the view eases a few yards ahead of
where the hero runs.

On a continent run, a gold needle under the banner points at the next zone as the camera sees it,
with the distance to that zone and the number of stops reached.

The run pauses itself when the pad drops out or the machine sleeps, and the pad rumbles when the
hero is hit (where the controller supports it).

On a handheld, `SURVIVORS_MAX_FPS` caps the frame rate and `SURVIVORS_UI_SCALE` scales the HUD and
the cards; the Switch launcher sets both.
