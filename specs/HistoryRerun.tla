--------------------------- MODULE HistoryRerun ---------------------------
\* Explicit state model for History Re-run provenance and readiness across
\* runner.rs (launch table written before spawn), bes.rs (unauthenticated BES
\* stream), and InvocationView.svelte / invocation.ts (rerunState).
\*
\* Two invocation IDs ("A", "B") may either be launched by Zelith (which writes
\* its trusted launch flags to SQLite before spawning Bazel) or arrive from an
\* external / forged local BES client (which can write BEP events with arbitrary
\* flags, including late-arriving flags during Refresh, but can never write the
\* launch table).
\*
\* RerunState is derived solely from selection, row kind, runnability, and the
\* trusted launch record (or Zelith-owned diagnostic). Unauthenticated BES
\* events can never make Re-run ready or influence launched flags.

Ids == {"A", "B"}
Rows == {"missing", "live", "diagnostic", "savedConsole", "saved"}
LaunchFlags == {"none", "emptyFlags", "customFlags"}
BesFlags == {"none", "emptyFlags", "customFlags", "forgedFlags"}
NoLaunch == [id |-> "none", flags |-> "none"]

VARIABLES id, row, runnable, launch, besFlags, launched, launchTick
vars == <<id, row, runnable, launch, besFlags, launched, launchTick>>

RerunState ==
    IF ~runnable \/ row[id] \in {"missing", "live", "savedConsole"} THEN "unavailable"
    ELSE IF row[id] = "diagnostic" /\ launch[id] # "none" THEN "ready"
    ELSE IF row[id] = "saved" /\ launch[id] # "none" THEN "ready"
    ELSE "unavailable"

RerunFlags == launch[id]

Init ==
    /\ id = "A"
    /\ row = [i \in Ids |-> "missing"]
    /\ runnable \in BOOLEAN
    /\ launch = [i \in Ids |-> "none"]
    /\ besFlags = [i \in Ids |-> "none"]
    /\ launched = NoLaunch
    /\ launchTick = FALSE

\* Zelith runner::start records trusted launch flags in SQLite BEFORE spawning
\* Bazel as a live invocation.
LaunchZelith(i, f) ==
    /\ row[i] = "missing"
    /\ launch[i] = "none"
    /\ f \in {"emptyFlags", "customFlags"}
    /\ launch' = [launch EXCEPT ![i] = f]
    /\ row' = [row EXCEPT ![i] = "live"]
    /\ UNCHANGED <<id, runnable, besFlags, launched, launchTick>>

\* A BES stream (from Bazel or a forged local client) arrives or refreshes with
\* late-arriving BEP flags. BES never modifies the trusted launch table.
ReceiveBes(i, bf) ==
    /\ bf \in {"none", "emptyFlags", "customFlags", "forgedFlags"}
    /\ besFlags' = [besFlags EXCEPT ![i] = bf]
    /\ row' = [row EXCEPT ![i] =
         IF row[i] = "live" THEN "savedConsole"
         ELSE "saved"]
    /\ UNCHANGED <<id, runnable, launch, launched, launchTick>>

\* A Zelith-launched live invocation finishes: if BES already recorded it,
\* savedConsole becomes saved; otherwise it becomes a Zelith-owned diagnostic.
FinishZelith(i) ==
    /\ row[i] \in {"live", "savedConsole"}
    /\ launch[i] # "none"
    /\ row' = [row EXCEPT ![i] =
         IF row[i] = "savedConsole" THEN "saved"
         ELSE "diagnostic"]
    /\ UNCHANGED <<id, runnable, launch, besFlags, launched, launchTick>>

Select(nextId) ==
    /\ nextId # id
    /\ id' = nextId
    /\ UNCHANGED <<row, runnable, launch, besFlags, launched, launchTick>>

ToggleRunnable ==
    /\ runnable' = ~runnable
    /\ UNCHANGED <<id, row, launch, besFlags, launched, launchTick>>

ClickRerun ==
    /\ RerunState = "ready"
    /\ launched' = [id |-> id, flags |-> RerunFlags]
    /\ launchTick' = ~launchTick
    /\ UNCHANGED <<id, row, runnable, launch, besFlags>>

Next ==
    \/ \E i \in Ids, f \in {"emptyFlags", "customFlags"} : LaunchZelith(i, f)
    \/ \E i \in Ids, bf \in BesFlags : ReceiveBes(i, bf)
    \/ \E i \in Ids : FinishZelith(i)
    \/ \E i \in Ids : Select(i)
    \/ ToggleRunnable
    \/ ClickRerun

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ id \in Ids
    /\ row \in [Ids -> Rows]
    /\ runnable \in BOOLEAN
    /\ launch \in [Ids -> LaunchFlags]
    /\ besFlags \in [Ids -> BesFlags]
    /\ launched \in ({NoLaunch} \cup [id : Ids, flags : {"emptyFlags", "customFlags"}])
    /\ launchTick \in BOOLEAN

\* An external or forged BES invocation (no Zelith launch record) is never ready.
ExternalNeverReady ==
    launch[id] = "none" => RerunState = "unavailable"

\* A live or savedConsole row is never ready for Re-run.
LiveNeverReady ==
    row[id] \in {"live", "savedConsole"} => RerunState = "unavailable"

\* Unrunnable workspace/selection is never ready.
UnrunnableNeverReady ==
    ~runnable => RerunState = "unavailable"

\* A Zelith-launched diagnostic or saved row is ready whenever runnable.
TrustedSavedOrDiagnosticReady ==
    (runnable /\ row[id] \in {"saved", "diagnostic"} /\ launch[id] # "none")
        => RerunState = "ready"

\* Re-run never launches for an external/forged invocation, a live/console row,
\* or an unrunnable selection, and never replays forged BES flags.
NeverLaunchExternalOrLive ==
    [][launchTick' # launchTick =>
        runnable
        /\ row[id] \in {"saved", "diagnostic"}
        /\ launch[id] # "none"
        /\ launched'.flags # "forgedFlags"]_vars

\* Every Re-run launches with the exact trusted launch flags recorded by Zelith,
\* regardless of what (or when) BES events reported.
LaunchedFlagsMatchLaunchRecord ==
    [][launchTick' # launchTick =>
        launched'.id = id /\ launched'.flags = launch[id]]_vars

\* Late-arriving or forged BES events never mutate trusted launch flags.
BesNeverMutatesLaunch ==
    [][(\E i \in Ids, bf \in BesFlags : ReceiveBes(i, bf)) => launch' = launch]_vars
=============================================================================
