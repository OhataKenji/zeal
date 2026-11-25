----------------------- MODULE WorkspaceRegistration -----------------------
EXTENDS Naturals
CONSTANT MaxRevision
VARIABLES stage, input, saved, registry, last, revision, dto
vars == <<stage, input, saved, registry, last, revision, dto>>

\* Native registry (not the UI token/delivery protocol in Workspace.tla).
\* lib.rs open_workspace / restore_workspace; workspace.rs is_workspace,
\* load_saved and save; scope.rs Scope::validated. Two roots, including a
\* second spelling of a, two normalized scopes and two possible git origins.
Roots == {"a","b"}
Paths == Roots \cup {"aliasA"}
Canon(p) == IF p = "aliasA" THEN "a" ELSE p
Scopes == {"all","narrow"}
Origins == {"none","origin1","origin2"}
Inputs == [path : Paths, scope : Scopes]
Unavailable(reason) == [path |-> reason, scope |-> "none"]
Unused == Unavailable("missing")
Empty == [p \in Roots |-> [scope |-> "none", origin |-> "none"]]
Keys == {p \in Roots : registry[p].scope # "none"}
Startup == {"restoreOK","restoreBlocked"}
Init == /\ stage \in Startup /\ input = Unused
        /\ saved \in Inputs \cup {Unused,Unavailable("invalid")}
        /\ registry = Empty /\ last = "none" /\ revision = 0
        /\ dto = [rev |-> 0, preferred |-> "none", rows |-> [p \in {} |-> p]]

Register(req,o) ==
    /\ registry' = [registry EXCEPT ![Canon(req.path)] = [scope |-> req.scope, origin |-> o]]
    /\ last' = Canon(req.path)
\* Readable, parseable preference (path-only is a valid default scope);
\* environment may still deny app-data, marker, canonicalization or UTF-8.
Restore ==
    /\ stage \in Startup /\ stage' = "ready"
    /\ IF saved \in Inputs /\ stage = "restoreOK" THEN \E o \in Origins : Register(saved,o)
       ELSE UNCHANGED <<registry,last>>
    /\ UNCHANGED <<input,saved,revision,dto>>

\* One explicit open at a time. Invalid reasons denote the first failing
\* check after trim/home expansion. Origin/history refresh is best effort;
\* absence of an origin never disqualifies the path.
Invalid == {Unavailable(r) : r \in {"scope","notDir","noMarker","unresolvable","nonUTF8"}}
Begin(req) == /\ stage = "ready" /\ revision + 2 <= MaxRevision
              /\ input' = req /\ stage' = "validate"
              /\ UNCHANGED <<saved,registry,last,revision,dto>>
Validate ==
    /\ stage = "validate"
    /\ IF input \in Invalid
       THEN /\ stage' = "ready" /\ input' = Unused /\ UNCHANGED <<registry,last>>
       ELSE /\ stage' = "save" /\ UNCHANGED input /\ \E o \in Origins : Register(input,o)
    /\ UNCHANGED <<saved,revision,dto>>
\* Save runs after the core lock is released. Missing data dir / failed
\* write may retain an older preference, or leave unreadable JSON.
Save == /\ stage = "save" /\ stage' = "emit"
        /\ saved' \in {saved, Unavailable("invalid"), [path |-> Canon(input.path), scope |-> input.scope]}
        /\ UNCHANGED <<input,registry,last,revision,dto>>

\* state.rs CoreState::workspace_dtos and Core::dto: groups are numbered
\* by first distinct origin in path order; no origin yields no group.
NoGroup == 2
Group(p) == IF registry[p].origin = "none" THEN NoGroup
            ELSE IF p = "b" /\ "a" \in Keys THEN
                    IF registry["a"].origin \notin {"none",registry[p].origin} THEN 1 ELSE 0
                 ELSE 0
Snapshot == /\ stage \in {"emit","reply","ready"} /\ revision < MaxRevision
            /\ revision' = revision + 1
            /\ dto' = [rev |-> revision', preferred |-> last,
                       rows |-> [p \in Keys |-> [scope |-> registry[p].scope, group |-> Group(p)]]]
            /\ stage' = IF stage = "emit" THEN "reply" ELSE "ready"
            /\ input' = IF stage = "reply" THEN Unused ELSE input
            /\ UNCHANGED <<registry,last,saved>>
Next == Restore \/ (\E req \in Inputs \cup Invalid : Begin(req))
        \/ Validate \/ Save \/ Snapshot \/ (stage = "ready" /\ UNCHANGED vars)
Spec == Init /\ [][Next]_vars

PreferredRegistered == last = "none" \/ last \in Keys
InvalidCannotPublish == [][stage = "validate" /\ input \in Invalid =>
    UNCHANGED <<registry,last,saved,revision,dto>>]_vars
CanonicalUpsert == [][stage = "validate" /\ stage' = "save" =>
    /\ last' = Canon(input.path) /\ registry'[last'].scope = input.scope
    /\ \A p \in Roots \ {last'} : registry'[p] = registry[p]]_vars
RestoreOnlySaved == [][(stage \in Startup /\ stage' = "ready") =>
    (IF stage = "restoreOK" /\ saved \in Inputs
     THEN last' = Canon(saved.path) /\ registry'[last'].scope = saved.scope
     ELSE last' = "none" /\ registry' = Empty)]_vars
PreferenceCannotRollback == [][stage = "save" => UNCHANGED <<registry,last>>]_vars
SavedPair == [][saved' # saved /\ saved' \in Inputs =>
    stage = "save" /\ saved'.path = last /\ saved'.scope = registry[last].scope]_vars
SnapshotProjection == [][dto' # dto =>
    /\ revision' = revision + 1 /\ dto'.rev = revision' /\ dto'.preferred = last
    /\ DOMAIN dto'.rows = Keys
    /\ \A p \in Keys :
        /\ dto'.rows[p].scope = registry[p].scope
        /\ (dto'.rows[p].group = NoGroup) = (registry[p].origin = "none")
        /\ \A q \in Keys :
            (dto'.rows[p].group = dto'.rows[q].group) = (registry[p].origin = registry[q].origin)
    ]_vars
=============================================================================
