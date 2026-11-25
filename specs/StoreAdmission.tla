--------------------------- MODULE StoreAdmission ---------------------------
EXTENDS Naturals
CONSTANT Clients, Roots
VARIABLES target, phase, owner, disk, writes
vars == <<target, phase, owner, disk, writes>>

\* src-tauri/src/store/database.rs: Store::open takes static ACCESS before
\* inspecting *any* root. _access lives in Store through the caller's SQL.
\* One open and one representative write (refresh_workspace) per client.
\* disk records admission facts, not file bytes or journal mode. "orphan"
\* means absent main with any WAL/SHM/journal; "badShape" has version 3
\* but fails the required SELECT preparations. Sidecar/file safety and I/O
\* checks can fail independently. No other process, crash, or security proof.
DiskKinds == {"absent", "orphan", "empty", "old", "newer", "badShape", "v3"}
Holding == {"inspect", "fresh", "probe", "use"}
Final == {"ok", "error"}
Init == /\ target \in [Clients -> Roots]
        /\ phase = [c \in Clients |-> "waiting"]
        /\ owner = "none" /\ disk \in [Roots -> DiskKinds]
        /\ writes = [d \in Roots |-> {}]

SetPhase(c,p) == phase' = [phase EXCEPT ![c] = p]
Acquire(c) == /\ phase[c] = "waiting" /\ owner = "none"
              /\ owner' = c /\ SetPhase(c,"inspect") /\ UNCHANGED <<disk, writes>>
Inspect(c,d) ==
    /\ phase[c] = "inspect"
    /\ CASE disk[d] = "absent" ->
                /\ disk' = [disk EXCEPT ![d] = "empty"] /\ SetPhase(c,"fresh")
         [] disk[d] \in {"v3", "badShape"} ->
                /\ UNCHANGED disk /\ SetPhase(c,"probe")
         [] OTHER -> FALSE
    /\ UNCHANGED <<owner, writes>>
\* Creation leaves an existing file even if the schema transaction fails.
\* After commit, sync or preparation can still fail with the v3 schema intact.
Initialize(c,d) == /\ phase[c] = "fresh"
                   /\ disk' = [disk EXCEPT ![d] = "v3"] /\ SetPhase(c,"probe")
                   /\ UNCHANGED <<owner, writes>>
Probe(c,d) == /\ phase[c] = "probe" /\ disk[d] = "v3"
              /\ SetPhase(c,"use") /\ UNCHANGED <<disk, owner, writes>>
Finish(c,p) == SetPhase(c,p) /\ owner' = "none"
Use(c,d) == /\ phase[c] = "use" /\ Finish(c,"ok")
            /\ writes' = [writes EXCEPT ![d] = @ \cup {c}] /\ UNCHANGED disk
Fail(c) == /\ phase[c] \in Holding /\ Finish(c,"error") /\ UNCHANGED <<disk, writes>>
Step(c) == /\ UNCHANGED target
           /\ (Acquire(c) \/
               (owner = c /\ (Inspect(c,target[c]) \/ Initialize(c,target[c]) \/
                              Probe(c,target[c]) \/ Use(c,target[c]) \/ Fail(c))))
Done == \A c \in Clients : phase[c] \in Final
Next == (\E c \in Clients : Step(c)) \/ (Done /\ UNCHANGED vars)
Spec == Init /\ [][Next]_vars /\ WF_vars(Next)

GuardLifetime == {c \in Clients : phase[c] \in Holding} =
                 (IF owner = "none" THEN {} ELSE {owner})
OnlyValidatedUse == \A c \in Clients : phase[c] = "use" => disk[target[c]] = "v3"
CommittedExactlyOnce == \A d \in Roots :
    writes[d] = {c \in Clients : target[c] = d /\ phase[c] = "ok"}
FreshOnly == [][\A d \in Roots : disk'[d] # disk[d] =>
    \E c \in Clients : owner = c /\ target[c] = d /\
       (\/ disk[d] = "absent" /\ disk'[d] = "empty" /\ phase'[c] = "fresh"
        \/ disk[d] = "empty" /\ disk'[d] = "v3" /\ phase[c] = "fresh")]_vars
RefuseExisting == [][\A c \in Clients :
    phase[c] = "inspect" /\ disk[target[c]] \in {"orphan","empty","old","newer","badShape"}
    => phase'[c] \notin {"fresh","use"}]_vars
\* Finite non-recurring clients, assuming local operations eventually step.
AllReleased == <>(Done /\ owner = "none")
=============================================================================
