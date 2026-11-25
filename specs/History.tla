------------------------------ MODULE History ------------------------------
EXTENDS Naturals

\* One workspace object, not one path. Switching away retires this object;
\* opening the same path later creates another instance.
VARIABLES phase, live, outstanding, owed
vars == <<phase, live, outstanding, owed>>
Init == /\ phase = "idle" /\ live = TRUE /\ outstanding = 0 /\ owed = FALSE

Request == /\ live
           /\ phase' = IF phase = "idle" THEN "reading" ELSE "queued"
           /\ outstanding' = IF phase = "idle" THEN outstanding + 1 ELSE outstanding
           /\ owed' = (outstanding > 0)
           /\ UNCHANGED live

\* Success, failure, and a retry to pin a new selection have the same
\* scheduling semantics (the pin first requests a follow-up).
Reply == /\ outstanding > 0
         /\ IF live /\ phase = "queued"
               THEN /\ phase' = "reading" /\ outstanding' = outstanding - 1 + 1
               ELSE /\ phase' = "idle" /\ outstanding' = outstanding - 1
         /\ owed' = FALSE /\ UNCHANGED live

Retire == /\ live /\ live' = FALSE /\ owed' = FALSE /\ UNCHANGED <<phase, outstanding>>
Quiescent == /\ ~live /\ outstanding = 0 /\ UNCHANGED vars
Next == Request \/ Reply \/ Retire \/ Quiescent
Spec == Init /\ [][Next]_vars /\ WF_vars(Reply)

SingleFlight == outstanding \in 0..1
Accounting == live => /\ (phase = "idle") = (outstanding = 0)
                      /\ (phase = "queued") = owed
FollowUp == owed ~> ~owed
ServeQueued == [][live /\ live' /\ owed /\ ~owed' =>
                   phase' = "reading" /\ outstanding' = 1]_vars
=============================================================================
