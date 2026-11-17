# Invocation lifecycle model check

`InvocationLifecycle.tla` models the removal of completed Console
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

Rechecked after the simplification on 2026-11-18 with TLC `2026.11.15.002146`, revision `bc1b506`, on
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

Independent copies with deliberate defects produced the expected counterexamples
on 2026-11-16 (the lifecycle model is unchanged):

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
Rust panics and poisoned mutex recovery are not modeled. In particular, the
detached interactive worker's panic cleanup does not itself publish an event;
that path is not covered by the normal `Release`/delivery liveness claim.

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

## Content identity after simplification

`StartNavigation.tla` checks the replacement of the frontend navigation counter
with existing content object identity and persistent workspace-open intent.
With `MaxContexts = 3` and `MaxOpens = 2`, TLC checked 1,140 distinct states on
2026-11-18: the type invariant and all four action/temporal properties passed.
The tool/JDK versions are the same as above.

| Implementation                                              | Model                                                                     |
| ----------------------------------------------------------- | ------------------------------------------------------------------------- |
| `state.content` object identity                             | `context`; numbers identify distinct objects, not runtime counters        |
| Changed target/package selection assigns fresh content      | `SelectTarget` and `TargetInvalidates`                                    |
| Targets/query/analytics page change or invocation selection | `ShowPage` / `SelectInvocation` replace context                           |
| Explicit workspace-open request, including a failed request | `OpenWorkspace` advances `opened` without changing the selected workspace |
| Captures in `start()` and its post-await guards             | `Start` / `Reply` capture and compare context, open intent and workspace  |
| Unmount ignores pending results                             | `Dispose` makes `Reply` unable to navigate                                |

The three contexts allow A→B→A page, target and workspace histories. The target
domain includes two targets and a package. The model keeps one outstanding reply
and abstracts the returned invocation ID; the existing lifecycle model covers
multiple invocation IDs and superseded starts. A reply's new invocation content
identity is abstracted because there is no other outstanding reply in this
model. Start failure messages, actual Svelte effects and startup preference
restoration are covered by implementation tests, not this model. Weak fairness
assumes a pending reply eventually arrives.

Deliberately defective copies failed at the intended properties:

| Removed behavior                                               | Violated property   |
| -------------------------------------------------------------- | ------------------- |
| Compare captured content with current content                  | `LateReplyIgnored`  |
| Compare captured open intent with current open intent          | `LateReplyIgnored`  |
| Replace content identity when selecting another target/package | `TargetInvalidates` |

These were action-property violations, not syntax or type-check failures.
The model SHA-256 is
`f198da3829a47e2ac53dfe3def387e9f933b67ddf143710330d13f9b435b385c`;
the configuration SHA-256 is
`0a7f4fad690f039797306234a0d36249c4dcd5a0ccb66673e718f216cc0b4579`.

To reproduce, use the command above with `StartNavigation.tla` and
`StartNavigation.cfg` instead of the lifecycle files. The invariants inspect
state transitions; they do not prove that every source-level type is necessary.
The retained state and deletion decisions are documented in
[the ownership audit](./necessary-state.md).
