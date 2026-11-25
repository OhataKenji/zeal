----------------------------- MODULE TargetRead ----------------------------
EXTENDS Sequences
\* One staged configured/actions read after a selected target's static
\* detail is available. Replacing the target wrapper permanently retires
\* this job; leaving the page makes active() false only while away.
VARIABLES phase, page, current, mode, accepted, steps
vars == <<phase, page, current, mode, accepted, steps>>
Init == /\ phase = "start" /\ page = TRUE /\ current = TRUE /\ mode \in {"configured","actions"}
        /\ accepted = FALSE /\ steps = <<>>
First == /\ phase = "start" /\ page /\ current
         /\ phase' = IF mode = "actions" THEN "actions" ELSE "configuration"
         /\ steps' = <<phase'>> /\ UNCHANGED <<page, current, mode, accepted>>
NextStep(p,q) == /\ phase = p /\ page /\ current /\ phase' = q
                 /\ steps' = Append(steps,q) /\ UNCHANGED <<page, current, mode, accepted>>
Publish == /\ phase \in {"actions","outputs"} /\ current /\ page
           /\ phase' = "done" /\ accepted' = TRUE /\ UNCHANGED <<page, current, mode, steps>>
Abort == /\ phase # "done" /\ phase' = "done" /\ UNCHANGED <<page, current, mode, steps, accepted>>
Navigate == /\ page' = ~page /\ UNCHANGED <<phase, current, mode, steps, accepted>>
Replace == /\ current /\ current' = FALSE /\ UNCHANGED <<phase, page, mode, steps, accepted>>
Next == First \/ NextStep("configuration","transitions") \/ NextStep("transitions","outputs")
        \/ Publish \/ Abort \/ Navigate \/ Replace
Spec == Init /\ [][Next]_vars
CompleteOnly == accepted => IF mode = "actions" THEN steps = <<"actions">>
                            ELSE steps = <<"configuration","transitions","outputs">>
NoOrphanWork == [][~(current /\ page) => UNCHANGED <<steps, accepted>>]_vars
=============================================================================
