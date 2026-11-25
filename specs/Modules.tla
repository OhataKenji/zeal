------------------------------ MODULE Modules ------------------------------
\* Each operation (graph or explain) holds its workspace permit even if
\* the frontend that submitted it has been replaced.
CONSTANT Paths, Clients
VARIABLES held, running, closing
vars == <<held, running, closing>>
Init == /\ held = {} /\ running = [c \in Clients |-> "none"] /\ closing = FALSE
Acquire(c,p) == /\ ~closing /\ running[c] = "none" /\ p \notin held
                /\ held' = held \cup {p} /\ running' = [running EXCEPT ![c] = p]
                /\ UNCHANGED closing
Release(c) == /\ running[c] # "none" /\ held' = held \ {running[c]}
              /\ running' = [running EXCEPT ![c] = "none"] /\ UNCHANGED closing
Shutdown == /\ ~closing /\ closing' = TRUE /\ UNCHANGED <<running, held>>
Done == /\ closing /\ held = {} /\ UNCHANGED vars
Next == (\E c \in Clients, p \in Paths : Acquire(c,p)) \/
        (\E c \in Clients : Release(c)) \/ Shutdown \/ Done
Spec == Init /\ [][Next]_vars /\ \A c \in Clients : WF_vars(Release(c))
Exclusive == \A c,d \in Clients : running[c] = running[d] /\ running[c] # "none" => c = d
NoLeak == held = {running[c] : c \in Clients} \ {"none"}
Drain == closing ~> (held = {})
=============================================================================
