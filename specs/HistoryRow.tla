----------------------------- MODULE HistoryRow -----------------------------
\* One invocation ID can denote different builds in different workspaces.
\* Scope-filtered database replies and pushed core state arrive independently.
Owners == {"local", "foreign"}
VARIABLES saved, execution, diagnostic, sibling
vars == <<saved, execution, diagnostic, sibling>>
Init == /\ saved = "none" /\ execution = "none" /\ diagnostic = FALSE /\ sibling \in BOOLEAN
Push == /\ execution' \in Owners \cup {"none"} /\ sibling' \in BOOLEAN
        /\ UNCHANGED <<saved, diagnostic>>
History == /\ saved' \in Owners \cup {"none"} /\ diagnostic' \in BOOLEAN
           /\ UNCHANGED <<execution, sibling>>
Spec == Init /\ [][Push \/ History]_vars

Pin == execution = "local" \/
       (execution = "foreign" /\ sibling /\ saved # "local" /\ ~diagnostic)
PinnedOwner == IF Pin THEN execution ELSE "none"
UseSaved == saved # "none" /\ (~Pin \/ saved = PinnedOwner)
Candidate == IF UseSaved THEN saved ELSE PinnedOwner
UseDiagnostic == diagnostic /\ Candidate # "local"
RowOwner == IF UseDiagnostic THEN "local" ELSE Candidate
ControlOwner == IF UseDiagnostic THEN "none" ELSE
                IF Pin THEN execution ELSE
                IF UseSaved /\ saved = execution THEN execution ELSE "none"
ControlMatchesRow == ControlOwner # "none" => ControlOwner = RowOwner
LocalPrecedence == (diagnostic \/ execution = "local" \/ saved = "local") => RowOwner = "local"
DiagnosticNoControls == UseDiagnostic => ControlOwner = "none"
=============================================================================
