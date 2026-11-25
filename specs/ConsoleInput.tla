--------------------------- MODULE ConsoleInput ---------------------------
EXTENDS Naturals, Sequences
CONSTANT Limit
VARIABLES mode, bytes, flight, expected
vars == <<mode, bytes, flight, expected>>
Init == /\ mode = "write" /\ bytes = <<>> /\ flight = "none" /\ expected = <<>>
Input == /\ mode = "write" /\ Len(bytes) < Limit
         /\ \E b \in {0,1} : /\ bytes' = Append(bytes,b) /\ expected' = Append(expected,b)
         /\ UNCHANGED <<mode, flight>>
Send == /\ mode = "write" /\ Len(bytes) > 0 /\ flight = "none"
        /\ flight' = "current" /\ UNCHANGED <<mode, bytes, expected>>
\* Unit writes abstract a partial acknowledgement into one accepted byte.
Reply(result) == /\ flight # "none"
                 /\ bytes' = CASE flight # "current" -> bytes
                               [] result = "ok" -> Tail(bytes)
                               [] result = "error" -> <<>>
                               [] OTHER -> bytes
                 /\ expected' = IF flight = "old" THEN expected ELSE
                                  CASE result = "ok" -> Tail(expected)
                                    [] result = "error" -> <<>> [] OTHER -> expected
                 /\ flight' = "none" /\ UNCHANGED mode
Revoke(next) == /\ mode # "disposed" /\ mode # next
                /\ mode' = next /\ bytes' = <<>> /\ expected' = <<>>
                /\ flight' = IF flight = "none" THEN "none" ELSE "old"
Enable == /\ mode = "readonly" /\ mode' = "write" /\ UNCHANGED <<bytes, flight, expected>>
Quiet == /\ mode = "disposed" /\ flight = "none" /\ UNCHANGED vars
Next == Input \/ Send \/ Enable \/ Revoke("readonly") \/ Revoke("disposed")
        \/ (\E result \in {"ok", "error", "blocked"} : Reply(result)) \/ Quiet
Spec == Init /\ [][Next]_vars
QueueIntact == bytes = expected /\ Len(bytes) <= Limit
ReadOnlyEmpty == mode # "write" => bytes = <<>>
OldReplyCannotConsume == [][flight = "old" /\ flight' = "none" => bytes' = bytes]_vars
=============================================================================
