----------------------- MODULE InvocationLifecycle -----------------------
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS Ids, MaxStops, NoSnapshot, NoSignal
ASSUME /\ Ids # {}
       /\ Ids \subseteq {"a1", "a2", "b1"}
       /\ MaxStops \in Nat

Workspaces == {"A", "B"}
Home(i) == IF i = "b1" THEN "B" ELSE "A"
None == "none"
Phases == {"unused", "requested", "running", "draining", "finished"}
EmptyEntries == [i \in {} |-> 0]
Snapshot(revision, entries) == [revision |-> revision, entries |-> entries]
Without(entries, i) == [j \in DOMAIN entries \ {i} |-> entries[j]]

\* phase and signal audit fields are model bookkeeping, not persisted data.
\* native is the proposed runtime ledger; removing an entry releases its
\* Console capture. Temporary read handles and byte contents are abstracted.
VARIABLES phase, native, latest, delayed, seen,
          workspace, selected, foreground, mayNavigate, readyReplies,
          stopQueue, stopIssued, signalCount, lastSignal, bepSuccess

vars == <<phase, native, latest, delayed, seen,
          workspace, selected, foreground, mayNavigate, readyReplies,
          stopQueue, stopIssued, signalCount, lastSignal, bepSuccess>>

Init ==
    /\ phase = [i \in Ids |-> "unused"]
    /\ native = EmptyEntries
    /\ latest = Snapshot(0, EmptyEntries)
    /\ delayed = NoSnapshot
    /\ seen = Snapshot(0, EmptyEntries)
    /\ workspace = "A"
    /\ selected = None
    /\ foreground = None
    /\ mayNavigate = FALSE
    /\ readyReplies = {}
    /\ stopQueue = {}
    /\ stopIssued = 0
    /\ signalCount = [i \in Ids |-> 0]
    /\ lastSignal = NoSignal
    /\ bepSuccess = {}

RequestStart(i) ==
    /\ phase[i] = "unused"
    /\ workspace = Home(i)
    /\ foreground = None
    /\ phase' = [phase EXCEPT ![i] = "requested"]
    /\ foreground' = i
    /\ mayNavigate' = TRUE
    /\ UNCHANGED <<native, latest, delayed, seen, workspace, selected,
                   readyReplies, stopQueue, stopIssued, signalCount,
                   lastSignal, bepSuccess>>

Start(i) ==
    /\ phase[i] = "requested"
    /\ phase' = [phase EXCEPT ![i] = "running"]
    /\ native' = native @@
         (i :> [workspace |-> Home(i), pgid |-> TRUE, stopping |-> FALSE])
    /\ latest' = Snapshot(latest.revision + 1, native')
    /\ readyReplies' = readyReplies \cup {i}
    /\ UNCHANGED <<delayed, seen, workspace, selected, foreground,
                   mayNavigate, stopQueue, stopIssued, signalCount,
                   lastSignal, bepSuccess>>

\* wait returned (success or error): revoke signal authority before drain.
End(i) ==
    /\ phase[i] = "running"
    /\ phase' = [phase EXCEPT ![i] = "draining"]
    /\ native' = [native EXCEPT ![i].pgid = FALSE]
    /\ UNCHANGED <<latest, delayed, seen, workspace, selected, foreground,
                   mayNavigate, readyReplies, stopQueue, stopIssued,
                   signalCount, lastSignal, bepSuccess>>

Release(i) ==
    /\ phase[i] = "draining"
    /\ phase' = [phase EXCEPT ![i] = "finished"]
    /\ native' = Without(native, i)
    /\ latest' = Snapshot(latest.revision + 1, native')
    /\ UNCHANGED <<delayed, seen, workspace, selected, foreground,
                   mayNavigate, readyReplies, stopQueue, stopIssued,
                   signalCount, lastSignal, bepSuccess>>

\* A start reply may arrive even after completion or A -> B -> A navigation.
Reply(i) ==
    /\ i \in readyReplies
    /\ readyReplies' = readyReplies \ {i}
    /\ selected' = IF foreground = i /\ mayNavigate THEN i ELSE selected
    /\ foreground' = IF foreground = i THEN None ELSE foreground
    /\ mayNavigate' = IF foreground = i THEN FALSE ELSE mayNavigate
    /\ UNCHANGED <<phase, native, latest, delayed, seen, workspace,
                   stopQueue, stopIssued, signalCount, lastSignal, bepSuccess>>

Switch(w) ==
    /\ w # workspace
    /\ workspace' = w
    /\ selected' = None
    /\ foreground' = None
    /\ mayNavigate' = FALSE
    /\ UNCHANGED <<phase, native, latest, delayed, seen, readyReplies,
                   stopQueue, stopIssued, signalCount, lastSignal, bepSuccess>>

Select(i) ==
    /\ Home(i) = workspace
    /\ i \in (DOMAIN seen.entries) \cup bepSuccess
    /\ i # selected
    /\ selected' = i
    /\ mayNavigate' = FALSE
    /\ UNCHANGED <<phase, native, latest, delayed, seen, workspace,
                   foreground, readyReplies, stopQueue, stopIssued,
                   signalCount, lastSignal, bepSuccess>>

Browse ==
    /\ selected # None
    /\ selected' = None
    /\ mayNavigate' = FALSE
    /\ UNCHANGED <<phase, native, latest, delayed, seen, workspace,
                   foreground, readyReplies, stopQueue, stopIssued,
                   signalCount, lastSignal, bepSuccess>>

\* Wrong workspace requests are also allowed as inputs to the native boundary.
QueueStop(i, w) ==
    /\ stopIssued < MaxStops
    /\ i \in DOMAIN seen.entries
    /\ stopIssued' = stopIssued + 1
    /\ stopQueue' = stopQueue \cup
         {[id |-> i, workspace |-> w, serial |-> stopIssued + 1]}
    /\ UNCHANGED <<phase, native, latest, delayed, seen, workspace,
                   selected, foreground, mayNavigate, readyReplies,
                   signalCount, lastSignal, bepSuccess>>

Stop(r) ==
    /\ r \in stopQueue
    /\ LET target == r.id
           allowed == IF r.id \in DOMAIN native
                      THEN native[r.id].workspace = r.workspace
                           /\ native[r.id].pgid /\ ~native[r.id].stopping
                      ELSE FALSE
       IN /\ native' = IF allowed
                       THEN [native EXCEPT ![target].stopping = TRUE]
                       ELSE native
          /\ signalCount' = IF allowed
                            THEN [signalCount EXCEPT ![target] = @ + 1]
                            ELSE signalCount
          /\ lastSignal' = IF allowed
              THEN [id |-> target, requestedId |-> r.id,
                    requestedWorkspace |-> r.workspace,
                    owner |-> native[target].workspace,
                    livePgid |-> native[target].pgid]
              ELSE lastSignal
    /\ stopQueue' = stopQueue \ {r}
    /\ latest' = Snapshot(latest.revision + 1, native')
    /\ UNCHANGED <<phase, delayed, seen, workspace, selected, foreground,
                   mayNavigate, readyReplies, stopIssued, bepSuccess>>

\* Arbitrary one delayed snapshot plus the latest models delivery reordering.
\* Duplicate/stale delivery is harmless; no unbounded network queue is stored.
HoldSnapshot ==
    /\ delayed = NoSnapshot
    /\ delayed' = latest
    /\ UNCHANGED <<phase, native, latest, seen, workspace, selected,
                   foreground, mayNavigate, readyReplies, stopQueue,
                   stopIssued, signalCount, lastSignal, bepSuccess>>

Deliver(s) ==
    /\ seen' = IF s.revision > seen.revision THEN s ELSE seen
    /\ UNCHANGED <<phase, native, latest, delayed, workspace, selected,
                   foreground, mayNavigate, readyReplies, stopQueue,
                   stopIssued, signalCount, lastSignal, bepSuccess>>

DeliverLatest == /\ latest.revision > seen.revision
                 /\ Deliver(latest)
DeliverDelayed == /\ delayed # NoSnapshot
                  /\ Deliver(delayed)

\* BEP build success is independent of the lifetime of a `bazel run` process.
BepSucceeded(i) ==
    /\ phase[i] \in {"running", "draining", "finished"}
    /\ i \notin bepSuccess
    /\ bepSuccess' = bepSuccess \cup {i}
    /\ UNCHANGED <<phase, native, latest, delayed, seen, workspace,
                   selected, foreground, mayNavigate, readyReplies,
                   stopQueue, stopIssued, signalCount, lastSignal>>

LiveSelection ==
    IF selected \in DOMAIN seen.entries
    THEN seen.entries[selected].workspace = workspace
    ELSE FALSE

View == IF selected = None THEN "workspace"
        ELSE IF LiveSelection THEN "console" ELSE "result"
DisplayStatus ==
    IF LiveSelection
    THEN IF seen.entries[selected].stopping THEN "stopping" ELSE "running"
    ELSE IF selected \in bepSuccess THEN "succeeded" ELSE "unknown"

Next ==
    \/ \E i \in Ids : RequestStart(i) \/ Start(i) \/ End(i) \/ Release(i)
                     \/ Reply(i) \/ Select(i) \/ BepSucceeded(i)
    \/ \E w \in Workspaces : Switch(w)
    \/ \E i \in Ids, w \in Workspaces : QueueStop(i, w)
    \/ \E r \in stopQueue : Stop(r)
    \/ Browse \/ HoldSnapshot \/ DeliverLatest \/ DeliverDelayed

StoppedEnd(i) ==
    /\ i \in DOMAIN native
    /\ native[i].stopping
    /\ End(i)

Spec ==
    /\ Init /\ [][Next]_vars
    /\ WF_vars(DeliverLatest)
    /\ \A i \in Ids : WF_vars(Start(i)) /\ WF_vars(Reply(i))
                     /\ WF_vars(StoppedEnd(i)) /\ WF_vars(Release(i))

EntriesTyped(entries) ==
    /\ DOMAIN entries \subseteq Ids
    /\ \A i \in DOMAIN entries :
         entries[i] \in [workspace : Workspaces, pgid : BOOLEAN, stopping : BOOLEAN]
SnapshotTyped(s) ==
    /\ s.revision \in 0..(2 * Cardinality(Ids) + MaxStops)
    /\ EntriesTyped(s.entries)
TypeOK ==
    /\ phase \in [Ids -> Phases]
    /\ EntriesTyped(native)
    /\ SnapshotTyped(latest) /\ SnapshotTyped(seen)
    /\ IF delayed = NoSnapshot THEN TRUE ELSE SnapshotTyped(delayed)
    /\ workspace \in Workspaces
    /\ selected \in Ids \cup {None} /\ foreground \in Ids \cup {None}
    /\ mayNavigate \in BOOLEAN /\ readyReplies \subseteq Ids
    /\ stopIssued \in 0..MaxStops
    /\ stopQueue \subseteq [id : Ids, workspace : Workspaces, serial : 1..MaxStops]
    /\ signalCount \in [Ids -> 0..MaxStops]
    /\ bepSuccess \subseteq Ids

ActiveLedger ==
    DOMAIN native = {i \in Ids : phase[i] \in {"running", "draining"}}
OwnerCaptured ==
    \A i \in DOMAIN native : native[i].workspace = Home(i)
DrainingRevokesPgid ==
    \A i \in DOMAIN native : phase[i] = "draining" => ~native[i].pgid
SafeSignals ==
    IF lastSignal = NoSignal THEN TRUE
    ELSE /\ lastSignal.id = lastSignal.requestedId
         /\ lastSignal.requestedWorkspace = lastSignal.owner
         /\ lastSignal.livePgid
OneInterrupt == \A i \in Ids : signalCount[i] <= 1
SelectionScoped == selected = None \/ Home(selected) = workspace
RunningDisplay == LiveSelection => DisplayStatus \in {"running", "stopping"}

RevisionMonotone == [][seen'.revision >= seen.revision]_vars
NoBackgroundNavigation ==
    [][(native' # native \/ latest' # latest \/ seen' # seen
        \/ bepSuccess' # bepSuccess
        \/ (stopQueue' \subseteq stopQueue /\ stopQueue' # stopQueue))
       => UNCHANGED <<workspace, selected>>]_vars
LateReplyIgnored ==
    [][\A i \in readyReplies :
         (i \notin readyReplies' /\ (foreground # i \/ ~mayNavigate))
         => selected' = selected]_vars
StaleReplyPreservesForeground ==
    [][\A i \in readyReplies :
         (i \notin readyReplies' /\ foreground # i)
         => foreground' = foreground]_vars
StopIsolated ==
    [][\A r \in stopQueue : r \notin stopQueue' =>
         \A j \in DOMAIN native \ {r.id} :
           j \in DOMAIN native' /\ native'[j] = native[j]]_vars
CompletedView ==
    \A i \in Ids :
      (phase[i] = "finished" /\ selected = i)
      ~> (selected # i \/ View = "result")
StoppedReleased ==
    \A i \in Ids :
      (i \in DOMAIN native /\ native[i].stopping) ~> (i \notin DOMAIN native)

=============================================================================
