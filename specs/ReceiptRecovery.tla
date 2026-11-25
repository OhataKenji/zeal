-------------------------- MODULE ReceiptRecovery --------------------------
EXTENDS Naturals
\* One invocation across process restarts. SQLite transactions are atomic;
\* flushed files survive a process exit (not a disk failure or power loss).
CONSTANT MaxAttempts
VARIABLES phase, attempt, files, complete, committed, acked
vars == <<phase, attempt, files, complete, committed, acked>>
Init == /\ phase = "offline" /\ attempt = 0 /\ files = {}
        /\ complete = FALSE /\ committed = {} /\ acked = {}
Boot == /\ phase = "offline" /\ phase' = "idle"
        /\ UNCHANGED <<attempt, files, complete, committed, acked>>
Reserve == /\ phase = "idle" /\ ~complete /\ attempt < MaxAttempts
           /\ attempt' = attempt + 1 /\ phase' = "recording"
           /\ UNCHANGED <<files, complete, committed, acked>>
Flush == /\ phase = "recording" /\ phase' = "flushed"
         /\ files' = files \cup {attempt} /\ UNCHANGED <<attempt, complete, committed, acked>>
\* Normal EOF may commit and ACK even without last_message. Only a sealed
\* receipt prevents a retry from reserving another attempt.
Commit == /\ phase = "flushed" /\ phase' = "saved" /\ complete' \in BOOLEAN
          /\ committed' = committed \cup {attempt} /\ UNCHANGED <<attempt, files, acked>>
Replay == /\ phase = "idle" /\ complete /\ phase' = "replay"
          /\ UNCHANGED <<attempt, files, complete, committed, acked>>
Ack == /\ phase \in {"saved","replay"} /\ phase' = "idle" /\ acked' = acked \cup {attempt}
       /\ UNCHANGED <<attempt, files, complete, committed>>
Crash == /\ phase # "offline" /\ phase' = "offline"
         /\ UNCHANGED <<attempt, files, complete, committed, acked>>
Next == Boot \/ Reserve \/ Flush \/ Commit \/ Replay \/ Ack \/ Crash
Spec == Init /\ [][Next]_vars
VisibleComplete == complete => attempt \in committed /\ attempt \in files
AckImpliesCommit == acked \subseteq committed /\ committed \subseteq files
ReplayReadOnly == [][complete => UNCHANGED <<complete, attempt, committed, files>>]_vars
\* A pre-commit crash can leave an orphan file but cannot select it as a
\* completed result. Reservation on the next connection chooses a fresh id.
NoOrphanPromotion == [][~complete /\ complete' =>
                       phase = "flushed" /\ attempt' = attempt]_vars
=============================================================================
