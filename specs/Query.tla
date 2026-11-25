------------------------------- MODULE Query -------------------------------
\* Flight is the transport's outstanding call; eligible records whether
\* an edit or unmount occurred since dispatch (even A -> B -> A).
VARIABLES phase, flight, eligible, publication
vars == <<phase, flight, eligible, publication>>
Init == /\ phase = "idle" /\ flight = FALSE /\ eligible = FALSE /\ publication = "none"
Run == /\ phase \in {"idle", "ready", "failed"}
       /\ phase' = "running" /\ flight' = TRUE /\ eligible' = TRUE /\ publication' = "none"
Edit == /\ phase # "disposed"
        /\ phase' = IF phase \in {"running", "draining"} THEN "draining" ELSE "idle"
        /\ eligible' = FALSE /\ publication' = "none" /\ UNCHANGED flight
Reply == /\ flight
         /\ \E result \in {"ready", "failed"} :
             /\ phase' = CASE phase = "running" -> result
                           [] phase = "disposed" -> "disposed"
                           [] OTHER -> "idle"
             /\ publication' = IF phase' \in {"ready", "failed"}
                               THEN IF eligible THEN "current" ELSE "stale"
                               ELSE publication
         /\ flight' = FALSE /\ eligible' = FALSE
Dispose == /\ phase # "disposed" /\ phase' = "disposed" /\ eligible' = FALSE
           /\ UNCHANGED <<flight, publication>>
Quiet == /\ phase = "disposed" /\ ~flight /\ UNCHANGED vars
Next == Run \/ Edit \/ Reply \/ Dispose \/ Quiet
Spec == Init /\ [][Next]_vars /\ WF_vars(Reply)
NoStaleResult == publication # "stale"
SlotMatchesTransport == phase # "disposed" =>
                          (phase \in {"running", "draining"}) = flight
EventuallyReleased == flight ~> ~flight
=============================================================================
