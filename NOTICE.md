# Notice, attribution and non-commercial statement

## Valve owns the map

Counter-Strike 2, `de_nuke`, the Source 2 engine and every asset derived from
them are the property of **Valve Corporation**. This project claims no ownership
of any of it, is not affiliated with, endorsed by, or connected to Valve in any
way, and is not an official Counter-Strike product.

Counter-Strike, Source and Valve are trademarks of Valve Corporation.

## Not commercial

This project makes no money and is not intended to. There is no advertising, no
sponsorship, no donations, no paid tier, no affiliate link and nothing for sale.
It will not be monetised.

It exists for **information and education**: to look at how a game map that is
dressed as a nuclear power station compares with a real one, and to be honest
about where it matches and where it does not.

## What is published here

The hosted viewer serves one derived data file, `de_nuke.plant.nkp.gz`. It holds
**untextured geometry** — vertex positions, normals and placement matrices — for
the subset of the map that depicts plant equipment: pipework, ducting, vessels,
machinery, instrumentation and the switchyard.

It deliberately does **not** contain:

- any texture, image or material artwork from the game,
- any sound, model file, script or map file from the game,
- the building fabric, props, clutter or anything else outside the plant subset,
- anything at all that could be loaded back into Counter-Strike.

Every surface renders as a flat colour chosen by this project to say what a
piece of geometry *is*. Nothing here reproduces the way the map looks in the
game, and nothing here substitutes for owning it.

The desktop build ships no game data whatsoever. It reads the map out of your
own Counter-Strike 2 installation, on your own machine, and writes nothing back.

## If Valve would rather this were not published

Open an issue, or contact the repository owner, and the data file will be taken
down. No argument. The source code is independent of it: the desktop viewer
works from a local CS2 install without any of it.

## Everything else

The code in this repository is the project's own work, under `MIT OR
Apache-2.0`, except:

- `crates/vendor/vpk` and `crates/vendor/source2`, copied from the **mapview**
  project under the same licence — see [`crates/vendor/README.md`](crates/vendor/README.md).

Reference photographs linked from the viewer are hosted on **Wikimedia Commons**
under their own licences. They are linked, never copied or redistributed here;
following a link takes you to the file page where the author and licence are
stated.

## This is not legal advice

The statements above describe what this project is and is not doing. They are a
description of intent and practice, not a claim to have permission for anything.
