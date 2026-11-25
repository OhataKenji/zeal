---------------------------- MODULE ShutdownJoin ----------------------------
\* runner.rs: spawn, interrupt, observe_process, InvocationGuard::drop,
\* exit_stop; state.rs: ProcessGroup::revoke; modules.rs: ModulesOperation.
\* Two reusable slots and one modules guard. "Joined" means their map/set
\* entries are gone, not that an OS thread was joined or a child died.
\* One exit clock crosses kill_at then finish_at for ALL owners. Collapse
\* instants into before/at exit, worker poll and wait errors into revoke,
\* and condvar wakeups into rechecks. No signal delivery, PID reuse, panic,
\* scheduler fairness or real-time guarantee. Worker KILL timing is abstract.
CONSTANT Slots
VARIABLES exit, slot, stamp, moduleBusy, signals
vars == <<exit, slot, stamp, moduleBusy, signals>>
Held == {s \in Slots : slot[s] # "empty"}
Signalable == {s \in Slots : slot[s] = "owned"}
Joined == Held = {} /\ ~moduleBusy
Init == /\ exit = "open" /\ moduleBusy = FALSE /\ signals = {}
        /\ slot = [s \in Slots |-> "empty"] /\ stamp = [s \in Slots |-> "none"]

Spawn(s) == /\ exit = "open" /\ s \notin Held
            /\ slot' = [slot EXCEPT ![s] = "owned"] /\ signals' = {}
            /\ UNCHANGED <<exit, stamp, moduleBusy>>
Acquire == /\ exit = "open" /\ ~moduleBusy /\ moduleBusy' = TRUE /\ signals' = {}
           /\ UNCHANGED <<exit, slot, stamp>>
DropModule == /\ moduleBusy /\ moduleBusy' = FALSE /\ signals' = {}
              /\ UNCHANGED <<exit, slot, stamp>>
Stop(s) == /\ s \in Signalable /\ stamp[s] = "none"
           /\ stamp' = [stamp EXCEPT ![s] = "beforeExit"] /\ signals' = {s}
           /\ UNCHANGED <<exit, slot, moduleBusy>>
WorkerKill(s) == /\ s \in Signalable /\ stamp[s] # "none" /\ signals' = {s}
                 /\ UNCHANGED <<exit, slot, stamp, moduleBusy>>
Observe(s) == /\ slot[s] \in {"owned", "unconfirmed"}
              /\ \E result \in {"reaped", "unconfirmed"} :
                    slot' = [slot EXCEPT ![s] = result]
              /\ signals' = {} /\ UNCHANGED <<exit, stamp, moduleBusy>>
Release(s) == /\ slot[s] \in {"reaped", "unconfirmed"}
              /\ slot' = [slot EXCEPT ![s] = "empty"]
              /\ stamp' = [stamp EXCEPT ![s] = "none"] /\ signals' = {}
              /\ UNCHANGED <<exit, moduleBusy>>

BeginExit == /\ exit = "open" /\ exit' = "grace"
             /\ signals' = {s \in Signalable : stamp[s] = "none"}
             /\ stamp' = [s \in Slots |-> IF s \in signals' THEN "atExit" ELSE stamp[s]]
             /\ UNCHANGED <<slot, moduleBusy>>
KillAt == /\ exit = "grace" /\ ~Joined /\ exit' = "confirm"
          /\ signals' = Signalable /\ UNCHANGED <<slot, stamp, moduleBusy>>
FinishAt == /\ exit = "confirm" /\ ~Joined /\ exit' = "timedOut"
            /\ slot' = [s \in Slots |-> IF s \in Signalable THEN "unconfirmed" ELSE slot[s]]
            /\ signals' = {} /\ UNCHANGED <<stamp, moduleBusy>>
ReturnJoined == /\ exit \in {"grace", "confirm"} /\ Joined /\ exit' = "drained"
                /\ signals' = {} /\ UNCHANGED <<slot, stamp, moduleBusy>>
Next == Acquire \/ DropModule \/ BeginExit \/ KillAt \/ FinishAt \/ ReturnJoined
        \/ (\E s \in Slots : Spawn(s) \/ Stop(s) \/ WorkerKill(s) \/ Observe(s) \/ Release(s))
        \/ (exit \in {"drained", "timedOut"} /\ Joined /\ UNCHANGED vars)
Spec == Init /\ [][Next]_vars

SignalOnlyOwned == [][signals' \subseteq Signalable]_vars
AdmissionClosed == [][exit # "open" => Held' \subseteq Held /\ (moduleBusy' => moduleBusy)]_vars
StopTimePreserved == [][\A s \in Held \cap Held' :
    (stamp[s] # "none" \/ slot'[s] = "reaped") => stamp'[s] = stamp[s]]_vars
TimeoutIsNotJoin == [][exit # "timedOut" /\ exit' = "timedOut" =>
    exit = "confirm" /\ ~Joined /\ Held' = Held /\ moduleBusy' = moduleBusy /\
    {s \in Slots : slot'[s] = "reaped"} = {s \in Slots : slot[s] = "reaped"}]_vars
ExitResult == /\ (exit = "drained" => Joined)
              /\ (exit = "timedOut" => Signalable = {})
=============================================================================
