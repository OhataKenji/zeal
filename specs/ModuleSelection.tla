--------------------------- MODULE ModuleSelection --------------------------
CONSTANT Keys
VARIABLES phase, selected, requested, explanation, changed
vars == <<phase, selected, requested, explanation, changed>>
Init == /\ phase = "idle" /\ selected = "none" /\ requested = "none"
        /\ explanation = "none" /\ changed = FALSE
Select(k) == /\ phase # "disposed" /\ selected # k /\ selected' = k
             /\ phase' = IF phase = "explaining" THEN "draining" ELSE phase
             /\ explanation' = "none" /\ changed' = TRUE /\ UNCHANGED requested
Explain == /\ phase = "idle" /\ selected \in Keys /\ phase' = "explaining"
           /\ requested' = selected /\ explanation' = "none" /\ changed' = FALSE
           /\ UNCHANGED selected
Reply == /\ phase \in {"explaining","draining"} /\ phase' = "idle"
         /\ explanation' = IF phase = "explaining" THEN requested ELSE "none"
         /\ UNCHANGED <<selected, requested, changed>>
Load == /\ phase = "idle" /\ phase' = "loading"
        /\ UNCHANGED <<selected, requested, explanation, changed>>
Loaded == /\ phase = "loading" /\ phase' = "idle"
          /\ selected' \in {selected, "none"} /\ explanation' = "none"
          /\ UNCHANGED <<requested, changed>>
LoadFailed == /\ phase = "loading" /\ phase' = "idle"
              /\ UNCHANGED <<selected, requested, explanation, changed>>
Dispose == /\ phase # "disposed" /\ phase' = "disposed"
           /\ UNCHANGED <<selected, requested, explanation, changed>>
Quiet == /\ phase = "disposed" /\ UNCHANGED vars
Next == (\E k \in Keys \cup {"none"} : Select(k))
        \/ Explain \/ Reply \/ Load \/ Loaded \/ LoadFailed \/ Dispose \/ Quiet
Spec == Init /\ [][Next]_vars
ExplanationOwned == explanation # "none" => explanation = selected /\ ~changed
SlotHeld == [][phase = "draining" /\ phase' # "disposed" => phase' \in {"idle","draining"}]_vars
=============================================================================
