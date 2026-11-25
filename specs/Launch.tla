------------------------------- MODULE Launch -------------------------------
EXTENDS Naturals
CONSTANT Paths, Slots, Jobs
VARIABLES reads, workspace, live, launch, target, feedback, detached, focus
vars == <<reads, workspace, live, launch, target, feedback, detached, focus>>
Init == /\ reads = [s \in Slots |-> "free"] /\ workspace \in Paths /\ live = TRUE
        /\ launch = [j \in Jobs |-> "idle"] /\ target = [j \in Jobs |-> workspace]
        /\ feedback = "none" /\ detached = {} /\ focus = "none"
Busy(path) == \E s \in Slots : reads[s] = path
Read(s) == /\ live /\ reads[s] = "free"
           /\ reads' = [reads EXCEPT ![s] = workspace]
           /\ UNCHANGED <<workspace, live, launch, target, feedback, detached, focus>>
FinishRead(s) == /\ reads[s] # "free" /\ reads' = [reads EXCEPT ![s] = "free"]
                 /\ UNCHANGED <<workspace, live, launch, target, feedback, detached, focus>>
Submit(j) == /\ live /\ feedback = "none" /\ launch[j] = "idle"
             /\ launch' = [launch EXCEPT ![j] = "waiting"]
             /\ target' = [target EXCEPT ![j] = workspace]
             /\ feedback' = j /\ detached' = detached \ {j}
             /\ UNCHANGED <<reads, workspace, live, focus>>
Outstanding == {j \in Jobs : launch[j] # "idle"}
Navigate == /\ live /\ workspace' \in Paths /\ feedback' = "none" /\ focus' = "none"
            /\ detached' = Outstanding /\ UNCHANGED <<reads, live, launch, target>>
Dispatch(j) == /\ live /\ launch[j] = "waiting" /\ ~Busy(target[j])
               /\ launch' = [launch EXCEPT ![j] = "sent"]
               /\ UNCHANGED <<reads, workspace, live, target, feedback, detached, focus>>
Reply(j,ok) == /\ launch[j] = "sent" /\ launch' = [launch EXCEPT ![j] = "idle"]
               /\ feedback' = IF feedback = j THEN "none" ELSE feedback
               /\ focus' = IF live /\ ok /\ feedback = j THEN j ELSE focus
               /\ UNCHANGED <<reads, workspace, live, target, detached>>
Dispose == /\ live /\ live' = FALSE /\ feedback' = "none" /\ detached' = Outstanding
           /\ launch' = [j \in Jobs |-> IF launch[j] = "waiting" THEN "idle" ELSE launch[j]]
           /\ UNCHANGED <<reads, workspace, target, focus>>
Quiet == /\ ~live /\ Outstanding = {} /\ ~(\E p \in Paths : Busy(p)) /\ UNCHANGED vars
Next == (\E s \in Slots : Read(s) \/ FinishRead(s))
        \/ (\E j \in Jobs : Submit(j) \/ Dispatch(j) \/ Reply(j,TRUE) \/ Reply(j,FALSE))
        \/ Navigate \/ Dispose \/ Quiet
Spec == Init /\ [][Next]_vars /\ \A j \in Jobs : WF_vars(Dispatch(j))
SafeDispatch == [][\A j \in Jobs : (launch[j] = "waiting" /\ launch'[j] = "sent")
                    => live /\ ~Busy(target[j])]_vars
SafeFocus == [][focus' # focus /\ focus' \in Jobs
                => live /\ focus' \notin detached /\ target[focus'] = workspace]_vars
\* Continuous discovery need not let a command run. Once submitted, if
\* its path stays quiet and the session stays mounted, dispatch is fair.
Unblocked == \A j \in Jobs : ~<>[](launch[j] = "waiting" /\ ~Busy(target[j]))
=============================================================================
