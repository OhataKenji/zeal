-------------------------- MODULE LaunchAdmission --------------------------
\* Join the two real protocols at the IPC boundary: sent is not spawned.
EXTENDS FiniteSets
CONSTANT Paths, Slots, Jobs
ASSUME Cardinality(Jobs) = 1
VARIABLES reads, workspace, live, launch, target, feedback, detached, focus,
          receiver, shutdown, process, signalled, decision, observedReady
UI == INSTANCE Launch
Core == INSTANCE Invocation
uiVars == <<reads, workspace, live, launch, target, feedback, detached, focus>>
coreVars == <<receiver, shutdown, process, signalled>>
vars == <<uiVars, coreVars, decision, observedReady>>
Init == UI!Init /\ Core!Init /\ decision = "none" /\ observedReady = FALSE

\* The view may submit against its last snapshot after the receiver failed.
Observe == /\ observedReady' = (receiver \in Core!Listening)
           /\ UNCHANGED <<uiVars, coreVars, decision>>

Client == /\ ((\E s \in Slots : UI!Read(s) \/ UI!FinishRead(s))
              \/ UI!Navigate \/ UI!Dispose
              \/ (\E j \in Jobs : UI!Dispatch(j) \/
                    (observedReady /\ UI!Submit(j))))
          /\ UNCHANGED <<coreVars, decision, observedReady>>
Server == /\ (Core!Bind \/ Core!Fail \/ Core!Stop \/ Core!Exit \/ Core!Escalate \/
              Core!Revoke \/ Core!Release \/ Core!CaptureFailed \/ Core!ExitKill)
          /\ UNCHANGED <<uiVars, decision, observedReady>>
Decide(j) == /\ launch[j] = "sent" /\ decision = "none"
             /\ ((Core!Start /\ decision' = "accept") \/
                 (UNCHANGED coreVars /\ decision' = "reject"))
             /\ UNCHANGED <<uiVars, observedReady>>
Deliver(j) == /\ decision # "none" /\ UI!Reply(j, decision = "accept")
              /\ decision' = "none" /\ UNCHANGED <<coreVars, observedReady>>
Next == Client \/ Server \/ Observe \/ (\E j \in Jobs : Decide(j) \/ Deliver(j))
Spec == Init /\ [][Next]_vars
AcceptedByLiveReceiver == [][decision # "accept" /\ decision' = "accept" =>
                             receiver \in Core!Listening /\ ~shutdown /\ process' = "running"]_vars
RejectedCannotFocus == [][decision = "reject" /\ decision' = "none" => focus' = focus]_vars
=============================================================================
