----------------------------- MODULE Clipboard -----------------------------
EXTENDS Naturals

\* One CopyButton instance (including QueryView's command/output buttons).
\* QueryView's separate 3s serialization-error notice is not clipboard state.
\* Two distinct attempts may capture equal text and settle in either order;
\* we model their feedback, not the order in which the OS changes its clipboard.
Writes == 1..2
Text == {"A", "B"}
Outcomes == {"copied", "failed"}
VARIABLES mounted, text, owner, captured, status, feedback, timer
vars == <<mounted, text, owner, captured, status, feedback, timer>>

Init == /\ mounted = TRUE /\ text \in Text /\ owner = 0
        /\ captured = [i \in Writes |-> "none"]
        /\ status = [i \in Writes |-> "unused"]
        /\ feedback = 0 /\ timer = 0

Edit == /\ mounted /\ text' \in Text
        /\ UNCHANGED <<mounted, owner, captured, status, feedback, timer>>

\* Starting a new attempt does NOT clear already published feedback.
Start(i) == /\ mounted /\ i = owner + 1
            /\ owner' = i /\ captured' = [captured EXCEPT ![i] = text]
            /\ status' = [status EXCEPT ![i] = "pending"]
            /\ UNCHANGED <<mounted, text, feedback, timer>>

\* Observe at effect-flush boundaries: publishing a result replaces the 1.5s
\* timeout (effect cleanup). A stale promise still settles, but cannot publish.
Settle(i, outcome) ==
    /\ status[i] = "pending"
    /\ status' = [status EXCEPT ![i] = outcome]
    /\ IF i = owner THEN /\ feedback' = i /\ timer' = i
       ELSE UNCHANGED <<feedback, timer>>
    /\ UNCHANGED <<mounted, text, owner, captured>>

Expire == /\ timer # 0 /\ feedback' = 0 /\ timer' = 0
          /\ UNCHANGED <<mounted, text, owner, captured, status>>
Unmount == /\ mounted /\ mounted' = FALSE /\ owner' = 0 /\ timer' = 0
           /\ UNCHANGED <<text, captured, status, feedback>>
Next == Edit \/ (\E i \in Writes : Start(i) \/ (\E r \in Outcomes : Settle(i, r)))
        \/ Expire \/ Unmount \/ (~mounted /\ UNCHANGED vars)
Spec == Init /\ [][Next]_vars

\* Returning to the same text can reveal a still-live result without a new
\* click. It is the text value, not a generation of props, that gates display.
Displayed == IF mounted /\ feedback # 0 THEN
                 IF captured[feedback] = text THEN status[feedback] ELSE "none"
             ELSE "none"
TimerOwned == timer = (IF mounted THEN feedback ELSE 0)
DisplayOwned ==
    Displayed # "none" =>
        /\ Displayed \in Outcomes /\ feedback <= owner /\ timer = feedback
        /\ \A i \in Writes : i > feedback => status[i] \notin Outcomes
IgnoredReplies ==
    [][(\E i \in Writes : i # owner /\ status[i] = "pending" /\ status'[i] \in Outcomes)
       => UNCHANGED <<feedback, timer>>]_vars
OwnerPublishes ==
    [][(\E i \in Writes : i = owner /\ status[i] = "pending" /\ status'[i] \in Outcomes)
       => feedback' = owner /\ timer' = owner /\
          Displayed' = (IF captured[owner] = text THEN status'[owner] ELSE "none")]_vars
=============================================================================
