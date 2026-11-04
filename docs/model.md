# Corridor model

Time is a nonnegative integer tick. Fixtures use abstract ticks. Input
conversions must choose a tick size before discretization. Every resource
has capacity one. The model validates positive run times, named resources,
unique trains, and resource-indexed fixed routes.

A train owns its current resource from entry until transfer or arrival.
If its next resource is unavailable, it continues holding the current
resource after completing service. Each resource also has a clearance
interval after release. Capacity uses half-open intervals. At one tick
the event queue processes service completions, closure ends, closure starts,
train releases and clearance wakes before dispatch. Closures forbid entry.
An active movement continues through a closure.

The dispatcher scores a ready train with its declared priority and the
time it has waited. Ties use waiting start and input train order. It uses
only current resource availability and waiting state. Weighted arrival
delay is minimized, and unfinished schedules have a classified failure.
The validator reconstructs per-train and per-resource sequences from the
emitted trace and checks them independently of the dispatcher.

The finite one-resource reference enumerates all permutations with
simultaneous release and positive weights. It certifies that small
subproblem. It supplies no optimality bound for the blocking corridor.
The abstraction is for offline research on fixed routes and excludes
signalling systems, traction dynamics and operational dispatch authority.
