---------------------------- MODULE Subscription ---------------------------
\* One mount. An unresolved listen() can hand back its cleanup function
\* AFTER unmount; that function must be invoked, not registered.
VARIABLES phase, live, installed
vars == <<phase, live, installed>>
Init == /\ phase = "new" /\ live = TRUE /\ installed = {}
InitCall == /\ live /\ phase \in {"new","listenFailed"} /\ phase' = "history"
            /\ UNCHANGED <<live, installed>>
Listen(which, next) ==
    /\ phase = which
    /\ phase' = IF live THEN next ELSE "disposed"
    /\ installed' = IF live THEN installed \cup {which} ELSE installed
    /\ UNCHANGED live
ListenFailed == /\ phase \in {"history","state"} /\ installed' = {}
                /\ phase' = IF live THEN "listenFailed" ELSE "disposed"
                /\ UNCHANGED live
ReadSnapshot == /\ phase = "snapshot" /\ phase' \in {"ready","snapshotFailed"}
                /\ UNCHANGED <<live, installed>>
RetrySnapshot == /\ live /\ phase = "snapshotFailed" /\ phase' = "snapshot"
                 /\ UNCHANGED <<live, installed>>
RecoverByPush == /\ live /\ phase = "snapshotFailed" /\ phase' = "ready"
                 /\ UNCHANGED <<live, installed>>
Dispose == /\ live /\ live' = FALSE /\ installed' = {}
           /\ phase' = IF phase \in {"history","state"} THEN phase ELSE "disposed"
Quiet == /\ phase \in {"ready","snapshotFailed","disposed"} /\ UNCHANGED vars
Next == InitCall \/ Listen("history","state") \/ Listen("state","snapshot") \/
        ListenFailed \/ ReadSnapshot \/ RetrySnapshot \/ RecoverByPush \/ Dispose \/ Quiet
Spec == Init /\ [][Next]_vars
Ordered == "state" \in installed => "history" \in installed
SnapshotAfterListeners == phase = "snapshot" => installed = {"history","state"}
UnmountClean == ~live => installed = {}
NoLateFetch == ~live => phase # "snapshot"
=============================================================================
