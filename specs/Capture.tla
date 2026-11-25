------------------------------- MODULE Capture -------------------------------
EXTENDS Naturals
\* terminal.rs: one successful spawn, its pty-output reader and its Capture.
\* Start at thread creation. Abstract bytes by flipping logBit on each write
\* (so even the second write is detectable). Interrupted reads and retries
\* before the drain are stutters. No clock, syscall success or panic model.
VARIABLES owner, handle, joins, done, draining, result, exited, failed,
          logBit, childLive, observed
vars == <<owner, handle, joins, done, draining, result, exited, failed,
          logBit, childLive, observed>>
worker == <<draining, result, exited, failed, logBit>>
capture == <<owner, handle, joins, done>>
child == <<childLive, observed>>

Init == /\ owner = "open" /\ handle = "some" /\ joins = 0 /\ done = FALSE
        /\ draining = FALSE /\ result = "reading" /\ exited = FALSE
        /\ failed = FALSE /\ logBit = FALSE /\ childLive = TRUE /\ observed = FALSE

\* Either finish or Drop::drop stores done. Neither decides the result.
\* The Option can be taken before OR after the reader returns. "joining"
\* denotes None in the field and a handle local to the in-flight call.
Begin(call) == /\ owner \in {"open", "returned"} /\ owner' = call /\ done' = TRUE
               /\ UNCHANGED <<handle, joins, worker, child>>
InCall == owner \in {"finish", "drop"}
Take == /\ InCall /\ handle = "some" /\ handle' = "joining"
        /\ UNCHANGED <<owner, joins, done, worker, child>>
ReturnTo == IF owner = "drop" THEN "dropped" ELSE "returned"
Join == /\ InCall /\ handle = "joining" /\ exited
        /\ handle' = "none" /\ joins' = joins + 1 /\ owner' = ReturnTo
        /\ UNCHANGED <<done, worker, child>>
Empty == /\ InCall /\ handle = "none" /\ owner' = ReturnTo
         /\ UNCHANGED <<handle, joins, done, worker, child>>

\* Only the worker's acquire load starts its drain interval. Data can still
\* be written during that interval. WouldBlock then stops immediately; the
\* elapsed-time test can stop even a continuously readable PTY. Neither
\* exit requires the child (or a descendant retaining the slave) to close it.
ObserveDone == /\ result = "reading" /\ done /\ ~draining /\ draining' = TRUE
               /\ UNCHANGED <<result, exited, failed, logBit, capture, child>>
Write == /\ result = "reading" /\ logBit' = ~logBit
         /\ UNCHANGED <<draining, result, exited, failed, capture, child>>
EndRead == /\ result = "reading"
           /\ result' \in ({"eof", "eio", "ioError"} \cup
                          (IF draining THEN {"wouldBlock", "deadline"} ELSE {}))
           /\ UNCHANGED <<draining, exited, failed, logBit, capture, child>>
\* ioError covers reader.read and log.write_all errors. Collapse the worker's
\* release store and return: join cannot complete until both have happened.
PublishReturn == /\ result # "reading" /\ ~exited /\ exited' = TRUE
                 /\ failed' = (result = "ioError")
                 /\ UNCHANGED <<draining, result, logBit, capture, child>>

\* runner.rs: poll_capture can acquire the failure flag while try_wait still
\* reports a live child. Observing it does not imply successful termination.
PollFailure == /\ owner = "open" /\ childLive /\ failed /\ ~observed /\ observed' = TRUE
               /\ UNCHANGED <<childLive, capture, worker>>
ChildExit == /\ childLive /\ childLive' = FALSE /\ UNCHANGED <<observed, capture, worker>>

Next == Begin("finish") \/ Begin("drop") \/ Take \/ Join \/ Empty
        \/ ObserveDone \/ Write \/ EndRead \/ PublishReturn \/ PollFailure \/ ChildExit
        \/ (owner = "dropped" /\ UNCHANGED vars)
\* Safety only: no scheduling, elapsed-time or I/O progress assumption.
Spec == Init /\ [][Next]_vars

SingleJoin == /\ joins \in 0..1 /\ (joins = 1 => exited)
              /\ (owner \in {"returned", "dropped"} => joins = 1)
DrainRequiresRequest == /\ (draining => done)
                        /\ (result \in {"wouldBlock", "deadline"} => draining)
FailureVisible == /\ (failed = (exited /\ result = "ioError"))
                  /\ ((owner = "open" /\ childLive /\ exited /\ result = "ioError" /\ ~observed)
                      => ENABLED PollFailure)
LogSealed == [][(joins = 1 \/ failed) => UNCHANGED logBit]_vars
RequestOnly == [][~done /\ done' => UNCHANGED worker]_vars
=============================================================================
