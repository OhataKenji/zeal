-------------------------- MODULE StartNavigation --------------------------
EXTENDS Naturals, FiniteSets

\* Context numbers represent distinct object identities, not runtime counters.
CONSTANTS MaxContexts, MaxOpens, None
Pages == {"targets", "query", "analytics", "invocation"}
Targets == {"first", "second", "package"}
Workspaces == {"A", "B"}
VARIABLES context, page, target, workspace, opened, pending, disposed
vars == <<context, page, target, workspace, opened, pending, disposed>>

Init ==
    /\ context = 1 /\ page = "targets" /\ target = "first"
    /\ workspace = "A" /\ opened = 0 /\ pending = None /\ disposed = FALSE

Start ==
    /\ ~disposed /\ pending = None
    /\ pending' = [context |-> context, opened |-> opened, workspace |-> workspace]
    /\ UNCHANGED <<context, page, target, workspace, opened, disposed>>

ShowPage(p) ==
    /\ ~disposed /\ p # page /\ context < MaxContexts
    /\ page' = p /\ context' = context + 1
    /\ UNCHANGED <<target, workspace, opened, pending, disposed>>

SelectTarget(t) ==
    /\ ~disposed /\ t # target /\ context < MaxContexts
    /\ target' = t /\ page' = "targets" /\ context' = context + 1
    /\ UNCHANGED <<workspace, opened, pending, disposed>>

SelectInvocation ==
    /\ ~disposed /\ context < MaxContexts
    /\ page' = "invocation" /\ context' = context + 1
    /\ UNCHANGED <<target, workspace, opened, pending, disposed>>

SwitchWorkspace(w) ==
    /\ ~disposed /\ w # workspace /\ context < MaxContexts
    /\ workspace' = w /\ page' = "targets" /\ context' = context + 1
    /\ UNCHANGED <<target, opened, pending, disposed>>

\* Explicit intent persists even if the open fails and no workspace changes.
OpenWorkspace ==
    /\ ~disposed /\ opened < MaxOpens
    /\ opened' = opened + 1
    /\ UNCHANGED <<context, page, target, workspace, pending, disposed>>

Reply ==
    /\ pending # None
    /\ page' = IF ~disposed /\ context = pending.context
                  /\ opened = pending.opened /\ workspace = pending.workspace
               THEN "invocation" ELSE page
    /\ pending' = None
    /\ UNCHANGED <<context, target, workspace, opened, disposed>>

Dispose ==
    /\ ~disposed /\ disposed' = TRUE
    /\ UNCHANGED <<context, page, target, workspace, opened, pending>>

Next ==
    \/ Start \/ Reply \/ OpenWorkspace \/ Dispose \/ SelectInvocation
    \/ (disposed /\ UNCHANGED vars)
    \/ \E p \in {"targets", "query", "analytics"} : ShowPage(p)
    \/ \E t \in Targets : SelectTarget(t)
    \/ \E w \in Workspaces : SwitchWorkspace(w)

Spec == Init /\ [][Next]_vars /\ WF_vars(Reply)
TypeOK ==
    /\ context \in 1..MaxContexts /\ page \in Pages /\ target \in Targets
    /\ workspace \in Workspaces /\ opened \in 0..MaxOpens
    /\ pending \in {None} \cup
         [context : 1..MaxContexts, opened : 0..MaxOpens, workspace : Workspaces]
    /\ disposed \in BOOLEAN

\* Track intent separately from identity to catch failure to replace the
\* content object on a target change (not just failure to compare it).
TargetInvalidates ==
    [][target' # target => context' # context]_vars
LateReplyIgnored ==
    [][(IF pending = None THEN FALSE
        ELSE pending' = None /\
             (disposed \/ context # pending.context
              \/ opened # pending.opened \/ workspace # pending.workspace))
       => page' = page]_vars
CurrentReplyNavigates ==
    [][(IF pending = None THEN FALSE
        ELSE pending' = None /\ ~disposed /\ context = pending.context
             /\ opened = pending.opened /\ workspace = pending.workspace)
       => page' = "invocation"]_vars
ReplySettles == pending # None ~> pending = None
=============================================================================
