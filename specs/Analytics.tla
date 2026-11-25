----------------------------- MODULE Analytics -----------------------------
\* Tokens can be reused only AFTER their previous transport call settles.
CONSTANT Tokens
VARIABLES pending, owner, latest, view
vars == <<pending, owner, latest, view>>
Init == /\ pending = {} /\ owner = "idle" /\ latest = "none"
        /\ view = "empty"
Load(t) == /\ owner # "disposed" /\ t \in Tokens \ pending
           /\ pending' = pending \cup {t} /\ owner' = t /\ latest' = t
           /\ view' = "loading"
Invalidate == /\ owner # "disposed" /\ owner' = "idle" /\ latest' = "none"
              /\ view' = "empty" /\ UNCHANGED pending
Reply(t) == /\ t \in pending /\ pending' = pending \ {t}
            /\ IF t = owner
               THEN /\ owner' = "idle" /\ view' \in {"ready", "failed"}
               ELSE UNCHANGED <<owner, view>>
            /\ UNCHANGED latest
Dispose == /\ owner # "disposed" /\ owner' = "disposed" /\ latest' = "none"
           /\ UNCHANGED <<pending, view>>
Quiet == /\ owner = "disposed" /\ pending = {} /\ UNCHANGED vars
Next == (\E t \in Tokens : Load(t) \/ Reply(t)) \/ Invalidate \/ Dispose \/ Quiet
Spec == Init /\ [][Next]_vars /\ \A t \in Tokens : WF_vars(Reply(t))
Owned == owner \in pending \cup {"idle", "disposed"}
NoStalePublication == [][\A t \in pending \ pending' :
                          t # latest => UNCHANGED <<owner, view>>]_vars
Frozen == [][owner = "disposed" => view' = view]_vars
Drain == (owner = "disposed") ~> (pending = {})
=============================================================================
