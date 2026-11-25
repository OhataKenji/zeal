--------------------------- MODULE BoundedProcess ---------------------------
EXTENDS Naturals
CONSTANT Deadline
VARIABLES now, cancel, wait, pipes, result, signalled
vars == <<now, cancel, wait, pipes, result, signalled>>
Streams == {"stdout","stderr"}
PipeStates(s) == [p \in Streams |-> s]

\* src-tauri/src/bazel_output.rs: run_bazel_bounded samples the flag before
\* spawn (not atomically with it); the deadline is supplied by the caller.
\* modules.rs passes its operation flag; workspace.rs currently supplies a
\* fresh false flag for info. This is the helper contract, with cancellation
\* unconstrained for either use, not an assertion that info has a stop API.
\* Deadline is one absolute logical instant shared by wait and both readers.
\* Byte caps, OS scheduling and the duration/success of kill/wait syscalls
\* are outside this finite control model. "ok" returns a status, not exit 0.
Init == /\ now \in 0..Deadline /\ cancel \in BOOLEAN
        /\ wait = "precheck" /\ pipes = PipeStates("none")
        /\ result = "pending" /\ signalled = FALSE
Tick == /\ now < Deadline /\ now' = now + 1
        /\ UNCHANGED <<cancel, wait, pipes, result, signalled>>
Cancel == /\ ~cancel /\ cancel' = TRUE /\ UNCHANGED <<now, wait, pipes, result, signalled>>
Check == /\ wait = "precheck"
         /\ wait' = IF cancel THEN "notSpawned" ELSE "spawn"
         /\ result' = IF cancel THEN "cancelled" ELSE result
         /\ UNCHANGED <<now, cancel, pipes, signalled>>
Spawn == /\ wait = "spawn"
         /\ (\/ wait' = "running" /\ pipes' = PipeStates("reading") /\ UNCHANGED result
             \/ wait' = "notSpawned" /\ result' = "spawnError" /\ UNCHANGED pipes)
         /\ UNCHANGED <<now, cancel, signalled>>

\* Independent readers: returning a status does not imply EOF (a descendant
\* may retain a writer); conversely both streams may close before child exit.
Read(p) == /\ pipes[p] = "reading"
           /\ \E s \in (IF now = Deadline THEN {"timeout"} ELSE {"eof","error"}) :
                  pipes' = [pipes EXCEPT ![p] = s]
           /\ UNCHANGED <<now, cancel, wait, result, signalled>>
\* try_wait's Some(status) and error precede the flag/deadline checks. Only
\* Ok(None) justifies the signal-and-reap attempt; cancel wins over timeout.
Poll == /\ wait = "running"
        /\ (\/ wait' \in {"exited","unknown"} /\ UNCHANGED signalled
            \/ (cancel \/ now = Deadline) /\ signalled' = TRUE
               /\ wait' = IF cancel THEN "cancelled" ELSE "timeout")
        /\ UNCHANGED <<now, cancel, pipes, result>>
Join == /\ wait \in {"exited","unknown","cancelled","timeout"}
        /\ \A p \in Streams : pipes[p] # "reading"
        /\ result' = CASE wait # "exited" -> wait
                        [] pipes["stdout"] # "eof" -> pipes["stdout"]
                        [] pipes["stderr"] # "eof" -> pipes["stderr"]
                        [] OTHER -> "ok"
        /\ UNCHANGED <<now, cancel, wait, pipes, signalled>>
Next == IF result = "pending"
        THEN Tick \/ Cancel \/ Check \/ Spawn \/ Poll \/ Join \/ (\E p \in Streams : Read(p))
        ELSE UNCHANGED vars
Spec == Init /\ [][Next]_vars /\ WF_vars(Next)

PrecancelRefused == [][wait = "precheck" /\ cancel => wait' # "spawn"]_vars
SignalOnlyUnreaped == [][~signalled /\ signalled' = TRUE =>
    wait = "running" /\ (cancel \/ now = Deadline)]_vars
NoReaderLeak == result # "pending" => \A p \in Streams : pipes[p] # "reading"
CompleteOutput == result = "ok" => wait = "exited" /\ pipes = PipeStates("eof")
SharedTimeout == (wait = "timeout" \/ (\E p \in Streams : pipes[p] = "timeout")) => now = Deadline
\* Weak progress assumes workers, time, and OS calls are not stuck. No EOF
\* fairness: a writer can stay open through the entire deadline.
Settles == <>(result # "pending")
=============================================================================
