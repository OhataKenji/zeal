---------------------------- MODULE GraphRenderer --------------------------
\* A render job, not a Worker object, owns the callbacks. Two jobs can
\* reuse the same warm worker. Saved cleanup functions may outlive either.
CONSTANT Jobs
VARIABLES phase, owner, issued, last, staleEffect
vars == <<phase, owner, issued, last, staleEffect>>
Init == /\ phase = "cold" /\ owner = "none" /\ issued = {}
        /\ last = "none" /\ staleEffect = FALSE
Start(j) == /\ phase # "disposed" /\ j \notin issued
            /\ phase' = "rendering" /\ owner' = j /\ issued' = issued \cup {j}
            /\ last' = j /\ staleEffect' = FALSE
Callback(j, next) == /\ j \in issued
                     /\ IF phase = "rendering" /\ owner = j
                        THEN /\ phase' = next /\ owner' = "none"
                             /\ staleEffect' = (j # last)
                        ELSE UNCHANGED <<phase, owner, staleEffect>>
                     /\ UNCHANGED <<issued, last>>
Dispose == /\ phase # "disposed" /\ phase' = "disposed" /\ owner' = "none"
           /\ UNCHANGED <<issued, last, staleEffect>>
Quiet == /\ phase = "disposed" /\ UNCHANGED vars
Next == (\E j \in Jobs : Start(j) \/ Callback(j, "warm") \/ Callback(j, "cold"))
        \/ Dispose \/ Quiet
Spec == Init /\ [][Next]_vars
OneOwner == (phase = "rendering") = (owner \in Jobs)
NoStaleEffect == ~staleEffect
Closed == [][phase = "disposed" => phase' = "disposed"]_vars
=============================================================================
