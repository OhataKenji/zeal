------------------------------- MODULE Reader -------------------------------
\* One slot identity. Owner invalidation is permanent, page eligibility
\* may return. A replaced wrapper starts a fresh instance with its cache.
VARIABLES phase, cached, active, retired, published
vars == <<phase, cached, active, retired, published>>
Init == /\ cached \in BOOLEAN /\ phase = IF cached THEN "ready" ELSE "empty"
        /\ active = TRUE /\ retired = FALSE /\ published = FALSE
Current == active /\ ~retired
Read == /\ Current /\ phase # "loading" /\ phase' = "loading" /\ published' = TRUE
        /\ UNCHANGED <<cached, active, retired>>
Success == /\ phase = "loading"
           /\ cached' = (Current \/ cached) /\ published' = Current
           /\ phase' = IF cached' THEN "ready" ELSE "empty"
           /\ UNCHANGED <<active, retired>>
Failure == /\ phase = "loading" /\ published' = Current
           /\ phase' = IF Current THEN "failed" ELSE IF cached THEN "ready" ELSE "empty"
           /\ UNCHANGED <<cached, active, retired>>
Navigate == /\ ~retired /\ active' = ~active /\ published' = FALSE
            /\ UNCHANGED <<phase, cached, retired>>
Retire == /\ ~retired /\ retired' = TRUE /\ published' = FALSE /\ UNCHANGED <<phase, cached, active>>
Quiet == /\ retired /\ phase # "loading" /\ UNCHANGED vars
Next == Read \/ Success \/ Failure \/ Navigate \/ Retire \/ Quiet
Spec == Init /\ [][Next]_vars /\ WF_vars(Success \/ Failure)
CacheState == (phase = "ready" => cached) /\ (phase = "empty" => ~cached)
OrphanSilent == retired => ~published
RetainOnFailure == [][phase = "loading" /\ phase' = "failed" => cached' = cached]_vars
Released == (phase = "loading") ~> (phase # "loading")
=============================================================================
