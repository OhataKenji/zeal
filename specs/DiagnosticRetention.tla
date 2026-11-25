------------------------ MODULE DiagnosticRetention ------------------------
EXTENDS Naturals, Sequences, FiniteSets
\* runner.rs: remember_diagnostic before InvocationGuard::drop. lib.rs:
\* load_invocation_history reads without the lock, settle_invocation_history
\* retires by RECORD owner, CoreState::diagnostics_for replies by REQUEST owner.
\* Two sibling workspaces, three terminal lifetimes (two reuse an id, never
\* while both are live); cache bound two instead of 32. One in-flight history
\* read returns zero/one repo-scoped record or an error. Store results are
\* inputs, not a model of SQLite, pin/window selection or monotone saves.
\* Exit includes unknown wait status. Assume readable console; omit metadata,
\* 16 KiB tail bytes, failed tail I/O, UI ordering and delivery after reply.
Workspaces == {"A", "B"}
Keys == {<<"A", 1>>, <<"B", 1>>, <<"A", 2>>}
Records == Workspaces \X {1, 2}
EmptyRead == [ok |-> TRUE, rows |-> {}]
FailedRead == [ok |-> FALSE, rows |-> {}]
Bound == 2
VARIABLES worker, cache, read, fetched, reply
vars == <<worker, cache, read, fetched, reply>>
Members(q) == {q[i] : i \in 1..Len(q)}
Live == {k \in Keys : worker[k] \in {"running", "saving", "releasing"}}
Init == /\ worker = [k \in Keys |-> "new"] /\ cache = <<>>
        /\ read = "idle" /\ fetched = EmptyRead /\ reply = {}

Start(k) == /\ worker[k] = "new" /\ \A other \in Live : other[2] # k[2]
            /\ worker' = [worker EXCEPT ![k] = "running"]
            /\ UNCHANGED <<cache, read, fetched, reply>>
\* read_record is keyed by id but may return the sibling's record. Missing,
\* inaccessible store and no data dir all take the same retention path.
ProcessExit(k) ==
    /\ worker[k] = "running"
    /\ \E found \in {{}} \cup {{r} : r \in {r \in Records : r[2] = k[2]}} :
          worker' = [worker EXCEPT ![k] = IF k \in found THEN "releasing" ELSE "saving"]
    /\ UNCHANGED <<cache, read, fetched, reply>>
Remember(k) ==
    /\ worker[k] = "saving"
    /\ cache' = <<k>> \o (IF Len(cache) = Bound THEN SubSeq(cache, 1, Bound - 1) ELSE cache)
    /\ worker' = [worker EXCEPT ![k] = "releasing"]
    /\ UNCHANGED <<read, fetched, reply>>
Release(k) == /\ worker[k] = "releasing" /\ worker' = [worker EXCEPT ![k] = "done"]
              /\ UNCHANGED <<cache, read, fetched, reply>>

\* Snapshot the store outcome, NOT the cache. A worker may remember a
\* diagnostic or drop its live slot before settlement takes the lock.
Read(w) == /\ read = "idle" /\ read' = w
           /\ fetched' \in {FailedRead, EmptyRead} \cup {[ok |-> TRUE, rows |-> {r}] : r \in Records}
           /\ UNCHANGED <<worker, cache, reply>>
Settle == /\ read \in Workspaces
          /\ cache' = IF ~fetched.ok THEN cache
                      ELSE SelectSeq(cache, LAMBDA k : k \notin fetched.rows)
          /\ reply' = {k \in Members(cache') : k[1] = read}
          /\ read' = "idle" /\ fetched' = EmptyRead /\ UNCHANGED worker
Next == (\E k \in Keys : Start(k) \/ ProcessExit(k) \/ Remember(k) \/ Release(k))
        \/ (\E w \in Workspaces : Read(w)) \/ Settle
Spec == Init /\ [][Next]_vars

BoundedRetention == /\ Len(cache) <= Bound /\ Len(cache) = Cardinality(Members(cache))
                    /\ Members(cache) \subseteq {k \in Keys : worker[k] \in {"releasing", "done"}}
NewestFirst == [][\A k \in Members(cache') \ Members(cache) :
    cache'[1] = k /\ SubSeq(cache', 2, Len(cache')) = SubSeq(cache, 1, Len(cache') - 1)
    /\ Len(cache') = (IF Len(cache) < Bound THEN Len(cache) + 1 ELSE Bound)]_vars
SettleExactly == [][(read \in Workspaces /\ read' = "idle") =>
    (\A k \in Keys :
        /\ (k \in Members(cache')) = (k \in Members(cache) /\ ~(fetched.ok /\ k \in fetched.rows))
        /\ (k \in reply') = (k \in Members(cache') /\ k[1] = read))
    ]_vars
=============================================================================
