--------------------------- MODULE HistoryRerun ---------------------------
\* Explicit state model for History Re-run readiness in InvocationView.svelte
\* downstream of SavedSelection. Two selectable invocation IDs ("A", "B") each
\* carry their own ground-truth saved flags on disk ("none" when the BEP file
\* is missing, unreadable, or lacks an original command line; "emptyFlags" when
\* no user flags were passed; "customFlags" when user flags were passed).
\*
\* RerunState is derived solely from current selection, row kind, runnability,
\* and the detail reader's phase/identity/flags. Clicking Re-run is enabled
\* only in state "ready".

Ids == {"A", "B"}
Rows == {"missing", "live", "diagnostic", "savedConsole", "saved"}
DetailPhases == {"empty", "loading", "ready", "failed"}
FlagValues == {"none", "emptyFlags", "customFlags"}
NoLaunch == [id |-> "none", flags |-> "none"]

VARIABLES id, row, runnable, detailId, detailPhase, detailFlags, trueFlags, retired, launched, launchTick
vars == <<id, row, runnable, detailId, detailPhase, detailFlags, trueFlags, retired, launched, launchTick>>

RerunState ==
    IF ~runnable \/ row \in {"missing", "live", "savedConsole"} THEN "unavailable"
    ELSE IF row = "diagnostic" THEN "ready"
    ELSE IF detailPhase = "loading" THEN "loading"
    ELSE IF detailPhase = "ready" /\ detailId = id /\ detailFlags # "none" THEN "ready"
    ELSE "unavailable"

RerunFlags ==
    IF row = "diagnostic" THEN "emptyFlags"
    ELSE detailFlags

Init ==
    /\ id = "A"
    /\ row = "missing"
    /\ runnable \in BOOLEAN
    /\ detailId = "none"
    /\ detailPhase = "empty"
    /\ detailFlags = "none"
    /\ trueFlags \in [Ids -> FlagValues]
    /\ retired = FALSE
    /\ launched = NoLaunch
    /\ launchTick = FALSE

Select(nextRow) ==
    /\ id' = IF id = "A" THEN "B" ELSE "A"
    /\ row' = nextRow
    /\ detailId' = "none"
    /\ detailFlags' = "none"
    /\ detailPhase' = IF nextRow = "saved" THEN "loading" ELSE "empty"
    /\ retired' = (retired \/ detailPhase = "loading")
    /\ UNCHANGED <<runnable, trueFlags, launched, launchTick>>

UpdateRow(nextRow) ==
    /\ nextRow # row
    /\ row' = nextRow
    /\ IF row = "saved" /\ nextRow # "saved" THEN
         /\ detailId' = "none"
         /\ detailFlags' = "none"
         /\ detailPhase' = "empty"
         /\ retired' = (retired \/ detailPhase = "loading")
       ELSE IF row # "saved" /\ nextRow = "saved" THEN
         /\ detailId' = "none"
         /\ detailFlags' = "none"
         /\ detailPhase' = "loading"
         /\ retired' = (retired \/ detailPhase = "loading")
       ELSE
         UNCHANGED <<detailId, detailFlags, detailPhase, retired>>
    /\ UNCHANGED <<id, runnable, trueFlags, launched, launchTick>>

\* Retained refresh while viewing a saved invocation (e.g., BEP still arriving):
\* detail cache stays visible, phase returns to "loading", Re-run disables, and
\* an initially missing BEP command line ("none") may now have arrived on disk.
Refresh(arrived) ==
    /\ row = "saved"
    /\ detailPhase # "loading"
    /\ arrived \in FlagValues
    /\ detailPhase' = "loading"
    /\ trueFlags' = IF trueFlags[id] = "none" THEN [trueFlags EXCEPT ![id] = arrived] ELSE trueFlags
    /\ retired' = (retired \/ detailPhase = "loading")
    /\ UNCHANGED <<id, row, runnable, detailId, detailFlags, launched, launchTick>>

\* User clicks Retry after a detail load failure.
Retry ==
    /\ row = "saved"
    /\ detailPhase = "failed"
    /\ detailPhase' = "loading"
    /\ UNCHANGED <<id, row, runnable, detailId, detailFlags, trueFlags, retired, launched, launchTick>>

DetailReply(ok) ==
    /\ detailPhase = "loading"
    /\ IF row # "saved" THEN
         /\ detailPhase' = "empty"
         /\ detailId' = "none"
         /\ detailFlags' = "none"
       ELSE IF ok THEN
         /\ detailPhase' = "ready"
         /\ detailId' = id
         /\ detailFlags' = trueFlags[id]
       ELSE
         /\ detailPhase' = "failed"
         /\ UNCHANGED <<detailId, detailFlags>>
    /\ UNCHANGED <<id, row, runnable, trueFlags, retired, launched, launchTick>>

LateReply ==
    /\ retired
    /\ retired' = FALSE
    /\ UNCHANGED <<id, row, runnable, detailId, detailPhase, detailFlags, trueFlags, launched, launchTick>>

ToggleRunnable ==
    /\ runnable' = ~runnable
    /\ UNCHANGED <<id, row, detailId, detailPhase, detailFlags, trueFlags, retired, launched, launchTick>>

ClickRerun ==
    /\ RerunState = "ready"
    /\ launched' = [id |-> id, flags |-> RerunFlags]
    /\ launchTick' = ~launchTick
    /\ UNCHANGED <<id, row, runnable, detailId, detailPhase, detailFlags, trueFlags, retired>>

Next ==
    \/ \E r \in Rows : Select(r)
    \/ \E r \in Rows : UpdateRow(r)
    \/ \E f \in FlagValues : Refresh(f)
    \/ Retry
    \/ \E ok \in BOOLEAN : DetailReply(ok)
    \/ LateReply
    \/ ToggleRunnable
    \/ ClickRerun

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ id \in Ids
    /\ row \in Rows
    /\ runnable \in BOOLEAN
    /\ detailId \in Ids \cup {"none"}
    /\ detailPhase \in DetailPhases
    /\ detailFlags \in FlagValues
    /\ trueFlags \in [Ids -> FlagValues]
    /\ retired \in BOOLEAN
    /\ launched \in [id: Ids \cup {"none"}, flags: FlagValues]
    /\ launchTick \in BOOLEAN
    /\ RerunState \in {"unavailable", "loading", "ready"}

LoadingDisablesRerun ==
    (row = "saved" /\ detailPhase = "loading") =>
        RerunState = IF runnable THEN "loading" ELSE "unavailable"

FailedDisablesRerun ==
    (row = "saved" /\ detailPhase = "failed") => RerunState = "unavailable"

MissingBepDisablesRerun ==
    (row = "saved" /\ detailPhase = "ready" /\ detailFlags = "none") => RerunState = "unavailable"

ReadyDetailMatchesSelection ==
    (row = "saved" /\ RerunState = "ready") =>
        (detailId = id /\ detailFlags = trueFlags[id] /\ detailFlags # "none")

NeverLaunchUnrunnableOrLive ==
    [][launchTick' # launchTick => runnable /\ row \in {"diagnostic", "saved"}]_vars

SavedFlagsMatchTruth ==
    [][launchTick' # launchTick /\ row = "saved" =>
        launched'.id = id /\ launched'.flags = trueFlags[id] /\ launched'.flags # "none"]_vars

LateReplySilent ==
    [][retired /\ ~retired' => UNCHANGED <<detailId, detailPhase, detailFlags, launched, launchTick>>]_vars
=============================================================================
