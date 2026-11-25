------------------------------ MODULE Workspace ----------------------------
EXTENDS Naturals
CONSTANT Paths, Requests, MaxRevision
VARIABLES calls, latest, revision, workspace, live, badPick
vars == <<calls, latest, revision, workspace, live, badPick>>
Init == /\ calls = {} /\ latest = "none" /\ revision = 0
        /\ workspace = "none" /\ live = TRUE /\ badPick = FALSE
\* The latest token is retained on failure: saved preferences never regain
\* ownership after an explicit open. Do not recycle a live token.
Open(t) == /\ live /\ t \notin calls /\ t # latest
           /\ calls' = calls \cup {t} /\ latest' = t
           /\ UNCHANGED <<revision, workspace, live, badPick>>
Restore(r,p) == IF r > revision /\ latest = "none" /\ workspace = "none"
                THEN p ELSE workspace
Push(r,p) == /\ live /\ revision' = IF r > revision THEN r ELSE revision
             /\ workspace' = Restore(r,p) /\ UNCHANGED <<calls, latest, live, badPick>>
Reply(t,r,p) == /\ t \in calls /\ calls' = calls \ {t}
                /\ revision' = IF live /\ r > revision THEN r ELSE revision
                /\ workspace' = IF live /\ latest = t THEN p ELSE workspace
                /\ badPick' = (workspace' # workspace /\ (latest # t \/ ~live))
                /\ UNCHANGED <<latest, live>>
Reject(t) == /\ t \in calls /\ calls' = calls \ {t}
             /\ UNCHANGED <<latest, workspace, revision, live, badPick>>
Dispose == /\ live /\ live' = FALSE /\ UNCHANGED <<calls, latest, workspace, revision, badPick>>
Quiet == /\ ~live /\ calls = {} /\ UNCHANGED vars
Next == (\E t \in Requests : Open(t) \/ Reject(t))
        \/ (\E p \in Paths, r \in 1..MaxRevision :
               Push(r,p) \/ (\E t \in Requests : Reply(t,r,p)))
        \/ Dispose \/ Quiet
Spec == Init /\ [][Next]_vars
LatestSelects == ~badPick
MonotoneSnapshot == [][revision' >= revision]_vars
NoRestoreAfterOpen == [][latest # "none" /\ calls' = calls => workspace' = workspace]_vars
Frozen == [][~live => UNCHANGED <<revision, workspace>>]_vars
=============================================================================
