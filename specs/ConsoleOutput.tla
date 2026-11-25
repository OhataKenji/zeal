---------------------------- MODULE ConsoleOutput ----------------------------
EXTENDS Naturals, FiniteSets
\* One mounted ConsoleView and its append-only console file. Two competing
\* callers stand for mount, visibility and timer ticks. Units distinguish
\* empty/short/full reads, not individual bytes or xterm rendering; 2 models
\* the 64 KiB cap. A growing file stops at 4 units. No fairness, EOF signal,
\* write/resize errors, or guarantee that a display promise settles.
Callers == {"timer", "view"}
VARIABLES length, offset, phase, start, limit, disposed, readError
vars == <<length, offset, phase, start, limit, disposed, readError>>
Busy == {p \in Callers : phase[p] # "idle"}

Init == /\ length = 0 /\ offset = 0 /\ disposed = FALSE /\ readError = FALSE
        /\ phase = [p \in Callers |-> "idle"]
        /\ start = [p \in Callers |-> 0] /\ limit = start

Grow == /\ length < 4 /\ length' = length + 1
        /\ UNCHANGED <<offset, phase, start, limit, disposed, readError>>
Poll(p) == /\ ~disposed /\ Busy = {}
           /\ phase' = [phase EXCEPT ![p] = "reading"]
           /\ start' = [start EXCEPT ![p] = offset]
           /\ UNCHANGED <<length, offset, limit, disposed, readError>>

\* Sample the file during the backend read. Delivery can be later, even
\* after disposal or more appends.
Capture(p) ==
    /\ phase[p] = "reading"
    /\ limit' = [limit EXCEPT ![p] = IF length > start[p] + 2
                                      THEN start[p] + 2 ELSE length]
    /\ phase' = [phase EXCEPT ![p] = "reply"]
    /\ UNCHANGED <<length, offset, start, disposed, readError>>

Display(p) ==
    /\ phase[p] = "reply" /\ ~disposed /\ limit[p] > start[p]
    /\ phase' = [phase EXCEPT ![p] = "display"]
    /\ UNCHANGED <<length, offset, start, limit, disposed, readError>>

Accept(p) ==
    /\ ~disposed
    /\ phase[p] = "display" \/ (phase[p] = "reply" /\ limit[p] = start[p])
    /\ offset' = limit[p] /\ readError' = FALSE
    /\ LET more == limit[p] = start[p] + 2 IN
       /\ phase' = [phase EXCEPT ![p] = IF more THEN "reading" ELSE "idle"]
       /\ start' = [start EXCEPT ![p] = IF more THEN offset' ELSE @]
    /\ UNCHANGED <<length, limit, disposed>>

\* A rejection from invoke or display releases the poll, without committing
\* the reply. Releasing an already disposed poll cannot report an error.
Fail(p) == /\ phase[p] \in {"reading", "display"}
           /\ phase' = [phase EXCEPT ![p] = "idle"]
           /\ readError' = IF disposed THEN readError ELSE TRUE
           /\ UNCHANGED <<length, offset, start, limit, disposed>>
Drop(p) == /\ disposed /\ phase[p] \in {"reply", "display"}
           /\ phase' = [phase EXCEPT ![p] = "idle"]
           /\ UNCHANGED <<length, offset, start, limit, disposed, readError>>
Dispose == /\ ~disposed /\ disposed' = TRUE
           /\ UNCHANGED <<length, offset, phase, start, limit, readError>>
Next == Grow \/ Dispose \/
        (\E p \in Callers : Poll(p) \/ Capture(p) \/ Display(p) \/
                            Accept(p) \/ Fail(p) \/ Drop(p)) \/
        (disposed /\ Busy = {} /\ UNCHANGED vars)
Spec == Init /\ [][Next]_vars

SinglePoll == Cardinality(Busy) <= 1
ReadFromCommit == offset <= length /\ \A p \in Busy : start[p] = offset
DisplayBacked == \A p \in Callers : phase[p] = "display" =>
                 start[p] < limit[p] /\ limit[p] <= length
NoFabricatedProgress ==
    [][offset' # offset =>
       \E p \in Busy : phase[p] = "display" /\ ~disposed /\
                       offset' = limit[p] /\ offset' > offset]_vars
DisposedSilent ==
    [][disposed => UNCHANGED <<offset, readError>> /\
        \A p \in Callers : phase'[p] = "display" => phase[p] = "display"]_vars
RetryAtFailedOffset ==
    [][readError' /\ (\E p \in Busy : phase'[p] = "idle") =>
       UNCHANGED offset]_vars
=============================================================================
