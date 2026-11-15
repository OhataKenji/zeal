# Invocation lifecycle model check

`InvocationLifecycle.tla` models the proposed removal of completed Console
retention. It checks a design, not the Rust/Svelte implementation.

The runtime ledger owns only active invocations, including their final output
drain. Reaping revokes signal authority; releasing the worker removes the whole
entry and its Console capture. The UI selects an invocation ID: an owned Console
is shown while present, otherwise the same invocation's BEP result is shown.
Completion does not change the selected workspace or invocation.

The model separates BEP build success from the lifetime of the local process.
An owned execution takes precedence over BEP success in the displayed status.

## Checked properties

- Ownership uses the workspace captured at launch, not the displayed workspace.
- Stop signals only its requested invocation in the matching workspace.
- Reaped executions cannot be signalled; repeated Stop does not resend SIGINT.
- No completed entry remains in the runtime ledger.
- Older state snapshots cannot replace newer snapshots.
- Late start replies cannot override navigation or another pending start.
- Backend updates cannot navigate away from the user's selection.
- Under the fairness assumptions below, a completed selected Console becomes a
  result view, and a stopped invocation is eventually released.

`phase`, `signalCount`, and `lastSignal` are verification bookkeeping. They do not
require production tombstones, retained results, or an event journal.

## Results

Checked on 2026-11-16 with TLC `2026.11.15.002146`, revision `bc1b506`, on
OpenJDK 21.0.13. All eight invariants and seven action/temporal properties passed.

| Configuration  | `Ids`          | `MaxStops` | Distinct states | Result |
| -------------- | -------------- | ---------: | --------------: | ------ |
| Two workspaces | `{"a1", "b1"}` |          2 |       4,116,568 | Pass   |
| Same workspace | `{"a1", "a2"}` |          1 |         646,686 | Pass   |
| Repeated Stop  | `{"a1"}`       |          2 |          13,516 | Pass   |

The checked model SHA-256 is
`de5919fba9be05e5b0930acf8a0badb8e0d08b8eeb202fb2d4b1d1574d6b7f4e`.
The tools JAR SHA-256 is
`7e53aa636dac8a3cb0533f343115c0548e03fc0ce5edc7739becf58d3d0fb84f`.

Independent copies with deliberate defects produced the expected counterexamples:

| Deliberate defect                                    | Violated property  |
| ---------------------------------------------------- | ------------------ |
| Retain a completed runtime entry                     | `ActiveLedger`     |
| Launch in the currently displayed workspace          | `OwnerCaptured`    |
| Stop the currently selected invocation               | `SafeSignals`      |
| Display BEP success before checking the live process | `RunningDisplay`   |
| Accept an older snapshot                             | `RevisionMonotone` |
| Navigate on an abandoned start reply                 | `SelectionScoped`  |
| Signal after reap revoked the process group          | `SafeSignals`      |
| Signal twice on repeated Stop                        | `OneInterrupt`     |

These were property violations, not parsing or evaluation failures. For example,
the Stop counterexample requests A while B is selected and signals B; the
reap counterexample signals an entry whose `pgid` capability is already false.

## Bounds and assumptions

Each modeled ID is used once. The model explores one arbitrary delayed snapshot
plus the latest snapshot, not an unbounded transport queue. Wrong-workspace Stop
requests are included. Successful spawn is assumed; spawn failure remains a
separate implementation-test concern.

Liveness assumes eventual delivery of the latest snapshot, processing of start
and reply actions, return from a stopped wait, and completion of the final drain.
Return from wait includes an unconfirmed exit: this is not proof that the OS
killed every descendant. The model does not check POSIX behavior, elapsed-time
bounds, Console bytes, BEP parsing, or the fixed BES capacity of 32.

Display properties refer to the latest **applied** client snapshot, not an
instantaneous view of the physical process. Transport delay can temporarily
leave the UI behind the backend. TLC uses fingerprints; the largest run reported
an optimistic collision-probability estimate of `6.4e-6`, not mathematical proof
over arbitrary executions or an unbounded number of invocations.

## Reproduce

Run from the repository root with a Java installation and `tla2tools.jar`.
Keep generated states and counterexample files outside the repository:

```sh
check_dir=$(mktemp -d)
cp docs/design/InvocationLifecycle.tla docs/design/InvocationLifecycle.cfg "$check_dir/"
cd "$check_dir"
java -Xmx4g -XX:+UseParallelGC -cp /path/to/tla2tools.jar tlc2.TLC \
  -workers 4 -metadir "$check_dir/states" \
  -config InvocationLifecycle.cfg InvocationLifecycle.tla
```

The committed configuration is the two-workspace case. For the other cases,
change only `Ids` and `MaxStops` in the copied configuration to the table values.
No state constraint, depth bound, or simulation mode was used for these runs.
