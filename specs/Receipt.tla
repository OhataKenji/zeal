------------------------------ MODULE Receipt ------------------------------
EXTENDS Naturals
CONSTANT Streams, Limit
VARIABLES stream, current, complete, superseded
vars == <<stream, current, complete, superseded>>
Empty == [phase |-> "unused", replay |-> FALSE, first |-> 0, last |-> 0,
          sealed |-> FALSE, flushed |-> FALSE, saved |-> FALSE, ack |-> 0, seen |-> {}]
Init == /\ stream = [s \in Streams |-> Empty] /\ current = "none"
        /\ complete = FALSE /\ superseded = {}

Begin(s) == /\ stream[s].phase = "unused"
            /\ stream' = [stream EXCEPT ![s].phase = "receive", ![s].replay = complete]
            /\ current' = IF complete THEN current ELSE s
            /\ superseded' = IF complete \/ current = "none" THEN superseded
                             ELSE superseded \cup {current}
            /\ UNCHANGED complete

\* A last_message seals Bazel payloads but not outer stream wrappers.
Append(s) == LET r == stream[s] IN
             /\ r.phase = "receive"
             /\ \E n \in 1..Limit, kind \in {"wrapper", "bazel", "last"} :
                 /\ IF r.first = 0 THEN r.replay \/ n = 1 ELSE n = r.last + 1
                 /\ ~r.sealed \/ kind = "wrapper"
                 /\ stream' = [stream EXCEPT ![s].first = IF r.first = 0 THEN n ELSE r.first,
                                             ![s].seen = r.seen \cup {n}, ![s].last = n,
                                             ![s].sealed = r.sealed \/ kind = "last"]
             /\ UNCHANGED <<current, complete, superseded>>

EOF(s) == /\ stream[s].phase = "receive" /\ stream[s].last > 0
          /\ stream' = [stream EXCEPT ![s].phase = "save", ![s].flushed = TRUE]
          /\ UNCHANGED <<current, complete, superseded>>

Commit(s) == /\ stream[s].phase = "save"
             /\ stream[s].replay \/ (current = s /\ ~complete)
             /\ stream' = [stream EXCEPT ![s].phase = "ack", ![s].saved = TRUE]
             /\ complete' = (complete \/ stream[s].sealed)
             /\ UNCHANGED <<current, superseded>>

QueueAck(s) == LET r == stream[s] IN
               /\ r.phase = "ack" /\ r.ack < r.last
               /\ stream' = [stream EXCEPT ![s].ack = IF r.ack = 0 THEN r.first ELSE r.ack + 1]
               /\ UNCHANGED <<current, complete, superseded>>

\* Error, empty EOF, or disconnect. Partial persistence may retain facts
\* but cannot promote received_complete or start the ack loop.
Close(s) == /\ stream[s].phase \in {"receive", "save", "ack"}
            /\ stream' = [stream EXCEPT ![s].phase = "closed"]
            /\ UNCHANGED <<current, complete, superseded>>
Done == /\ \A s \in Streams : stream[s].phase = "closed"
        /\ UNCHANGED vars
Next == (\E s \in Streams : Begin(s) \/ Append(s) \/ EOF(s) \/ Commit(s)
                            \/ QueueAck(s) \/ Close(s)) \/ Done
Spec == Init /\ [][Next]_vars
AckAfterSave == \A s \in Streams : LET r == stream[s] IN
                 r.ack > 0 => r.flushed /\ r.saved /\ r.ack \in r.first..r.last
RangeRules == \A s \in Streams : LET r == stream[s] IN
              r.last > 0 => r.first \in 1..r.last /\ (~r.replay => r.first = 1)
AckSeen == \A s \in Streams : stream[s].ack > 0 => stream[s].ack \in stream[s].seen
ContiguousAcks == [][\A s \in Streams : LET r == stream[s] IN
                      stream'[s].ack # r.ack =>
                      stream'[s].ack = (IF r.ack = 0 THEN r.first ELSE r.ack + 1)]_vars
CompleteHasWitness == complete => \E s \in Streams :
                      ~stream[s].replay /\ stream[s].saved /\ stream[s].sealed
NoStaleCommit == [][\A s \in superseded : stream'[s].saved = stream[s].saved]_vars
CompleteIsFinal == [][complete => complete' /\ current' = current]_vars
=============================================================================
