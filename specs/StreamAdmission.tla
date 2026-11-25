-------------------------- MODULE StreamAdmission --------------------------
EXTENDS Naturals, FiniteSets
\* src-tauri/src/bes.rs: Receiver::admit, Stream::{append,partial,finish},
\* receive_events and publish_build_tool_event_stream's spawned task. It
\* binds _stream THROUGH send_acks; a closed response is not a task abort.
\* Three one-shot clients, two permits (native 32), at most two events,
\* one buffered ACK per response (native channel 8). Model queueing, not
\* remote delivery. Idle/send timeouts are decisions, not 300s clocks.
\* No BES payloads, sequence validation, persistence internals, bounded
\* post-release send_error or runtime panics. A flush success represents
\* the file flush AND the successful receipt.finish database update.
CONSTANT Clients, Capacity
VARIABLES phase, free, events, sent, buffered
vars == <<phase, free, events, sent, buffered>>
Working == {"receive", "append", "detached", "partial", "finish", "ack"}
Owners == {c \in Clients : phase[c] \in Working}
Init == /\ phase = [c \in Clients |-> "new"] /\ free = Capacity
        /\ events = [c \in Clients |-> 0] /\ sent = [c \in Clients |-> 0]
        /\ buffered = [c \in Clients |-> FALSE]
Move(c, p) == phase' = [phase EXCEPT ![c] = p]
Step(c, p) == Move(c, p) /\ UNCHANGED <<free, events, sent, buffered>>
Drop(c, p) == Move(c, p) /\ free' = free + 1

TryAdmit(c) == /\ phase[c] = "new"
               /\ IF free = 0 THEN Step(c, "rejected")
                  ELSE /\ Move(c, "receive") /\ free' = free - 1
                       /\ UNCHANGED <<events, sent, buffered>>
Receive(c) == /\ phase[c] = "receive"
              /\ \/ (events[c] < 2 /\ Step(c, "append"))
                 \/ Step(c, "partial")  \* idle timeout, malformed request or input error
                 \/ (events[c] > 0 /\ Step(c, "finish"))  \* normal EOF
                 \/ (events[c] = 0 /\ Drop(c, "empty") /\ UNCHANGED <<events, sent, buffered>>)
Append(c) == /\ phase[c] = "append"
             /\ \/ Step(c, "detached") \* drop awaiting future, NOT blocking closure
                \/ Step(c, "partial")  \* append_event calls save_partial on append error
                \/ /\ Move(c, "receive") /\ events' = [events EXCEPT ![c] = @ + 1]
                   /\ UNCHANGED <<free, sent, buffered>>
WorkerDrop(c) == /\ phase[c] \in {"detached", "partial", "finish"}
                 /\ Drop(c, "closed")  \* includes failed finish; no ACK
                 /\ UNCHANGED <<events, sent, buffered>>
Saved(c) == phase[c] = "finish" /\ Step(c, "ack")

QueueAck(c) == /\ phase[c] = "ack" /\ ~buffered[c] /\ sent[c] < events[c]
               /\ sent' = [sent EXCEPT ![c] = @ + 1]
               /\ buffered' = [buffered EXCEPT ![c] = TRUE]
               /\ UNCHANGED events
               /\ IF sent'[c] = events[c] THEN Drop(c, "complete")
                  ELSE Move(c, "ack") /\ UNCHANGED free
TakeAck(c) == /\ buffered[c] /\ buffered' = [buffered EXCEPT ![c] = FALSE]
              /\ UNCHANGED <<phase, free, events, sent>>
EndAck(c) == /\ phase[c] = "ack"
             /\ (Drop(c, "disconnect") \/ (buffered[c] /\ Drop(c, "sendTimeout")))
             /\ UNCHANGED <<events, sent, buffered>>
Next == (\E c \in Clients : TryAdmit(c) \/ Receive(c) \/ Append(c) \/ WorkerDrop(c)
                            \/ Saved(c) \/ QueueAck(c) \/ TakeAck(c) \/ EndAck(c))
        \/ ((\A c \in Clients : phase[c] # "new") /\ Owners = {} /\ UNCHANGED vars)
Spec == Init /\ [][Next]_vars

PermitAccounting == free \in 0..Capacity /\ free + Cardinality(Owners) = Capacity
CancellationKeepsPermit == [][\A c \in Clients :
    phase[c] = "append" /\ phase'[c] = "detached" => free' = free]_vars
AckAfterSave == [][\A c \in Clients : sent'[c] > sent[c] =>
    phase[c] = "ack" /\ ~buffered[c] /\ c \in Owners]_vars
FlushBeforeAck == [][\A c \in Clients : phase[c] # "ack" /\ phase'[c] = "ack" =>
    phase[c] = "finish" /\ events[c] > 0]_vars
AckRange == \A c \in Clients :
    /\ sent[c] \in 0..events[c]
    /\ (sent[c] > 0 => phase[c] \in {"ack", "complete", "disconnect", "sendTimeout"})
    /\ (phase[c] = "complete" => sent[c] = events[c] /\ events[c] > 0)
=============================================================================
