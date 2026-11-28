---------------------------- MODULE DiagnosticFlow ----------------------------
\* Explicit end-to-end state model for Zelith's build-diagnostic navigation
\* across workspace pages, invocation selection, saved detail/profile readers,
\* and invocation comparison:
\*   Analytics -> History (selected invocation) -> Targets / Timing / Compare
\*
\* Models a finite set of invocations `Inv` where each invocation may be
\* running or settled, and verifies that:
\*   - saved detail/profile reads only exist for settled invocations,
\*   - comparison only loads/displays when the left invocation is settled and
\*     ready on the Compare tab,
\*   - leaving History or the Compare tab tears down comparison state cleanly,
\*   - drilling down from Analytics lands on History with a clean reader state.
EXTENDS FiniteSets

CONSTANTS Inv

None == "none"
Pages == {"targets", "history", "analytics", "workspace"}
Tabs == {"all", "targets", "details", "logs", "timing", "compare"}
ReaderPhases == {"empty", "loading", "ready", "failed"}
ComparePhases == {"idle", "loading", "ready", "failed"}

VARIABLES
  page,
  selected,
  tab,
  detail,
  profile,
  compare,
  compareTarget,
  running

vars == <<page, selected, tab, detail, profile, compare, compareTarget, running>>

TypeOK ==
  /\ page \in Pages
  /\ selected \in Inv \cup {None}
  /\ tab \in Tabs
  /\ detail \in ReaderPhases
  /\ profile \in ReaderPhases
  /\ compare \in ComparePhases
  /\ compareTarget \in Inv \cup {None}
  /\ running \subseteq Inv

Init ==
  /\ page = "history"
  /\ selected = None
  /\ tab = "all"
  /\ detail = "empty"
  /\ profile = "empty"
  /\ compare = "idle"
  /\ compareTarget = None
  /\ running = {}

ResetCompare ==
  /\ compare' = "idle"
  /\ compareTarget' = None

OpenPage(p) ==
  /\ p \in Pages
  /\ p # page
  /\ page' = p
  /\ IF p # "history"
     THEN ResetCompare
     ELSE UNCHANGED <<compare, compareTarget>>
  /\ UNCHANGED <<selected, tab, detail, profile, running>>

SelectInvocation(i) ==
  /\ i \in Inv
  /\ page \in {"history", "analytics"}
  /\ page' = "history"
  /\ selected' = i
  /\ IF selected # i
     THEN /\ detail' = "empty"
          /\ profile' = "empty"
          /\ tab' = "all"
          /\ ResetCompare
     ELSE /\ UNCHANGED <<detail, profile, tab>>
          /\ IF page # "history" THEN ResetCompare ELSE UNCHANGED <<compare, compareTarget>>
  /\ UNCHANGED running

SelectTab(t) ==
  /\ page = "history"
  /\ selected # None
  /\ t \in Tabs
  /\ t # tab
  /\ tab' = t
  /\ IF t # "compare"
     THEN ResetCompare
     ELSE UNCHANGED <<compare, compareTarget>>
  /\ UNCHANGED <<page, selected, detail, profile, running>>

StartDetail ==
  /\ page = "history"
  /\ selected # None
  /\ selected \notin running
  /\ detail \in {"empty", "failed"}
  /\ detail' = "loading"
  /\ UNCHANGED <<page, selected, tab, profile, compare, compareTarget, running>>

FinishDetail(ok) ==
  /\ detail = "loading"
  /\ detail' = IF ok THEN "ready" ELSE "failed"
  /\ UNCHANGED <<page, selected, tab, profile, compare, compareTarget, running>>

StartProfile ==
  /\ page = "history"
  /\ selected # None
  /\ selected \notin running
  /\ profile \in {"empty", "failed"}
  /\ profile' = "loading"
  /\ UNCHANGED <<page, selected, tab, detail, compare, compareTarget, running>>

FinishProfile(ok) ==
  /\ profile = "loading"
  /\ profile' = IF ok THEN "ready" ELSE "failed"
  /\ UNCHANGED <<page, selected, tab, detail, compare, compareTarget, running>>

AutoCompare(other) ==
  /\ page = "history"
  /\ tab = "compare"
  /\ selected # None
  /\ selected \notin running
  /\ detail = "ready"
  /\ compare = "idle"
  /\ other \in Inv
  /\ other # selected
  /\ other \notin running
  /\ compare' = "loading"
  /\ compareTarget' = other
  /\ UNCHANGED <<page, selected, tab, detail, profile, running>>

StartCompare(other) ==
  /\ page = "history"
  /\ tab = "compare"
  /\ selected # None
  /\ selected \notin running
  /\ detail = "ready"
  /\ compare # "loading"
  /\ other \in Inv
  /\ IF other \in running
     THEN /\ compare' = "failed"
          /\ compareTarget' = None
     ELSE /\ compare' = "loading"
          /\ compareTarget' = other
  /\ UNCHANGED <<page, selected, tab, detail, profile, running>>

FinishCompare(ok) ==
  /\ compare = "loading"
  /\ IF ok /\ compareTarget \notin running
     THEN /\ compare' = "ready"
          /\ UNCHANGED compareTarget
     ELSE /\ compare' = "failed"
          /\ compareTarget' = None
  /\ UNCHANGED <<page, selected, tab, detail, profile, running>>

StartRun(i) ==
  /\ i \in Inv
  /\ i \notin running
  /\ running' = running \cup {i}
  /\ IF selected = i
     THEN /\ detail' = "empty"
          /\ profile' = "empty"
          /\ ResetCompare
     ELSE IF compareTarget = i
     THEN /\ ResetCompare
          /\ UNCHANGED <<detail, profile>>
     ELSE UNCHANGED <<detail, profile, compare, compareTarget>>
  /\ UNCHANGED <<page, selected, tab>>

FinishRun(i) ==
  /\ i \in running
  /\ running' = running \ {i}
  /\ UNCHANGED <<page, selected, tab, detail, profile, compare, compareTarget>>

Next ==
  \/ \E p \in Pages : OpenPage(p)
  \/ \E i \in Inv : SelectInvocation(i)
  \/ \E t \in Tabs : SelectTab(t)
  \/ StartDetail
  \/ FinishDetail(TRUE)
  \/ FinishDetail(FALSE)
  \/ StartProfile
  \/ FinishProfile(TRUE)
  \/ FinishProfile(FALSE)
  \/ \E i \in Inv : AutoCompare(i)
  \/ \E i \in Inv : StartCompare(i)
  \/ FinishCompare(TRUE)
  \/ FinishCompare(FALSE)
  \/ \E i \in Inv : StartRun(i)
  \/ \E i \in Inv : FinishRun(i)

Spec == Init /\ [][Next]_vars

\* Safety invariants
SavedReadsOnlyWhenSettled ==
  (detail # "empty" \/ profile # "empty") =>
    (selected # None /\ selected \notin running)

CompareRequiresReadyLeft ==
  compare \in {"loading", "ready"} =>
    (page = "history" /\ tab = "compare" /\ selected # None /\ selected \notin running /\ detail = "ready")

CompareTargetSettled ==
  compare \in {"loading", "ready"} =>
    (compareTarget # None /\ compareTarget \notin running)

ComparisonMountedOnlyOnCompareTab ==
  (page # "history" \/ tab # "compare") =>
    (compare = "idle" /\ compareTarget = None)
=============================================================================
