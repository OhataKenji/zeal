---------------------------- MODULE Invocation ----------------------------
\* One receiver and one reusable invocation slot. Revocation is distinct
\* from release: a worker can still own output after the pid is gone.
VARIABLES receiver, shutdown, process, signalled
vars == <<receiver, shutdown, process, signalled>>
Live == {"running", "interrupt", "kill"}
Listening == {"preferred", "fallback"}
Init == /\ receiver = "starting" /\ shutdown = FALSE /\ process = "absent" /\ signalled = FALSE
Bind == /\ receiver = "starting" /\ receiver' \in Listening
        /\ UNCHANGED <<process, shutdown>> /\ signalled' = FALSE
Fail == /\ receiver # "unavailable" /\ receiver' = "unavailable"
        /\ UNCHANGED <<process, shutdown>> /\ signalled' = FALSE
Start == /\ receiver \in Listening /\ ~shutdown /\ process = "absent"
         /\ process' = "running" /\ UNCHANGED <<receiver, shutdown>> /\ signalled' = FALSE
Stop == /\ process = "running" /\ process' = "interrupt"
        /\ signalled' = TRUE /\ UNCHANGED <<receiver, shutdown>>
Exit == /\ ~shutdown /\ shutdown' = TRUE
        /\ process' = IF process = "running" THEN "interrupt" ELSE process
        /\ signalled' = (process = "running") /\ UNCHANGED receiver
Escalate == /\ process = "interrupt" /\ process' = "kill"
            /\ signalled' = TRUE /\ UNCHANGED <<receiver, shutdown>>
\* Worker and exit timers can each send KILL, including after the other.
ExitKill == /\ shutdown /\ process \in Live /\ signalled' = TRUE
            /\ UNCHANGED <<process, receiver, shutdown>>
CaptureFailed == /\ process \in Live /\ signalled' = TRUE
                 /\ UNCHANGED <<process, receiver, shutdown>>
Revoke == /\ process \in Live /\ process' = "draining"
          /\ signalled' = FALSE /\ UNCHANGED <<receiver, shutdown>>
Release == /\ process # "absent" /\ process' = "absent"
           /\ signalled' = FALSE /\ UNCHANGED <<receiver, shutdown>>
\* Missing, repeated or foreign stop: no signal.
Ignore == /\ signalled' = FALSE /\ UNCHANGED <<receiver, shutdown, process>>
Next == Bind \/ Fail \/ Start \/ Stop \/ Exit \/ Escalate \/ ExitKill \/ CaptureFailed
        \/ Revoke \/ Release \/ Ignore
Spec == Init /\ [][Next]_vars
StartGate == [][(process = "absent" /\ process' = "running")
                => receiver \in Listening /\ ~shutdown]_vars
NoRevokedSignal == [][process \notin Live => ~signalled']_vars
ShutdownGate == shutdown => process # "running"
=============================================================================
