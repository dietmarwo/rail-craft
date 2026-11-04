"""Synthetic corridor models shared by the Python examples.

The corridor has one capacity-one central block shared by eastbound and westbound trains.
Each direction enters through its own siding and leaves through its own platform, so
routes cannot form a circular wait. Times are abstract integer ticks.
"""

PRIORITIES = (0, 1, 3, 0, 2, 4)
SIDING, PLATFORM = 1, 2
WEST_SIDING, EAST_SIDING, CENTRAL, EAST_PLATFORM, WEST_PLATFORM = range(5)


def corridor(trains=12, *, block_run=4, clearance=1, headway=3, slack=4,
             closures=(), horizon=400):
    """Return a model JSON object with `trains` alternating eastbound and westbound trains.

    Pairs of opposing trains are released every `headway` ticks. A train's planned arrival
    is its unobstructed run time plus `slack`. `closures` is a list of `(start, end)` tick
    pairs for the central block.
    """
    resources = [{"name": name, "clearance": clearance} for name in
                 ("west siding", "east siding", "central block", "east platform", "west platform")]
    rows = []
    for i in range(trains):
        east = i % 2 == 0
        release = headway * (i // 2) + i % 2
        route = [
            {"resource": WEST_SIDING if east else EAST_SIDING, "duration": SIDING},
            {"resource": CENTRAL, "duration": block_run},
            {"resource": EAST_PLATFORM if east else WEST_PLATFORM, "duration": PLATFORM},
        ]
        rows.append({
            "name": f"{'E' if east else 'W'}{i:02d}",
            "release": release,
            "planned": release + SIDING + block_run + PLATFORM + slack,
            "priority": PRIORITIES[i % len(PRIORITIES)],
            "route": route,
        })
    return {
        "horizon": horizon,
        "resources": resources,
        "trains": rows,
        "closures": [{"resource": CENTRAL, "start": s, "end": e} for s, e in closures],
    }


LOOP_NAMES = ("west entry", "block A", "loop east", "loop west", "block B", "east exit",
              "east entry", "west exit")


def passing_loop(trains=8, *, headway=6, run=3, loop=2, slack=6, offset=1, horizon=400,
                 priorities=(2, 0, 1, 3, 0, 1)):
    """Return a single-track line with one passing loop.

    Eastbound trains run west entry, block A, loop east, block B, east exit; westbound
    trains run east entry, block B, loop west, block A, west exit. The greedy dispatcher
    has no look-ahead: if it lets opposing trains fill both blocks and both loop tracks,
    the four trains wait on each other and the schedule deadlocks. Which train wins a
    contended block depends on the dispatch weights.
    """
    resources = [{"name": name, "clearance": 1} for name in LOOP_NAMES]
    rows = []
    for i in range(trains):
        east = i % 2 == 0
        release = headway * (i // 2) + (i % 2) * offset
        legs = ([(0, 1), (1, run), (2, loop), (4, run), (5, 1)] if east
                else [(6, 1), (4, run), (3, loop), (1, run), (7, 1)])
        rows.append({
            "name": f"{'E' if east else 'W'}{i:02d}",
            "release": release,
            "planned": release + sum(d for _, d in legs) + slack,
            "priority": priorities[i % len(priorities)],
            "route": [{"resource": r, "duration": d} for r, d in legs],
        })
    return {"horizon": horizon, "resources": resources, "trains": rows, "closures": []}
