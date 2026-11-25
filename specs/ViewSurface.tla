----------------------------- MODULE ViewSurface -----------------------------
\* AppView after reactive effects settle. surface is the explicit owner; the
\* initial workspace form can be visible with no explicit owner at all.
VARIABLES workspace, surface, opening, edited, prevented, focusInput
vars == <<workspace, surface, opening, edited, prevented, focusInput>>
Owners == {"none", "palette", "workspace", "run"}
PaletteAllowed == workspace /\ surface \in {"none", "palette"}
Form == surface \in {"workspace", "run"} \/ ~workspace
Init == /\ workspace = FALSE /\ surface = "none" /\ opening = FALSE /\ edited = FALSE
        /\ prevented = FALSE /\ focusInput = FALSE

\* Transition to a fresh form, close the current one, or show the palette
\* and request input focus. The outputs describe requests, not DOM focus.
Show(s) == /\ surface' = s /\ edited' = FALSE /\ focusInput' = (s = "palette")
Keep == /\ UNCHANGED <<surface, edited>> /\ focusInput' = FALSE

\* k means Ctrl/Meta+K without shift/alt/repeat. ignored includes composing,
\* missing Ctrl/Meta, extra K modifiers/repeat, and R without shift.
\* runOK abstracts the package guard (true on the non-package branch).
Key(k, runOK) ==
    /\ UNCHANGED <<workspace, opening>>
    /\ prevented' = CASE k = "o" -> TRUE [] k = "r" -> workspace
                      [] k = "k" -> PaletteAllowed [] OTHER -> FALSE
    /\ CASE k = "o" /\ workspace /\ ~opening /\ surface # "workspace" -> Show("workspace")
         [] k = "r" /\ PaletteAllowed /\ runOK -> Show("run")
         [] k = "k" /\ PaletteAllowed -> Show(IF surface = "palette" THEN "none" ELSE "palette")
         [] OTHER -> Keep

\* By contrast with the O shortcut, an explicit pick can seed a new path.
PickPath == /\ ~opening /\ Show("workspace")
            /\ UNCHANGED <<workspace, opening, prevented>>
Toolbar == /\ PaletteAllowed
           /\ IF surface = "palette" THEN Keep ELSE Show("palette")
           /\ UNCHANGED <<workspace, opening, prevented>>
RunButton == /\ workspace /\ surface = "none" /\ Show("run")
             /\ UNCHANGED <<workspace, opening, prevented>>
Edit == /\ Form /\ edited' = TRUE /\ UNCHANGED <<workspace, surface, opening, prevented, focusInput>>

\* CommandPalette.choose closes before dispatch. Refresh/target/page/history
\* leave no overlay. An ineligible package run or busy open also leaves none.
Choose(action, allowed, accept) ==
    /\ surface = "palette"
    /\ IF accept THEN Show(CASE action = "run" /\ allowed -> "run"
                              [] action = "open" /\ ~opening -> "workspace"
                              [] OTHER -> "none")
       ELSE Keep
    /\ UNCHANGED <<workspace, opening, prevented>>

\* Owner is checked at delivery, not at the time a close was scheduled.
\* A palette native close also tests !dialog.open to survive rapid reopen.
Close(s, reopened) ==
    /\ IF surface = s /\ ~(s = "palette" /\ reopened)
       THEN Show("none") ELSE Keep
    /\ UNCHANGED <<workspace, opening, prevented>>

BeginOpen == /\ ~opening /\ opening' = TRUE /\ UNCHANGED <<workspace, surface, edited, prevented, focusInput>>
FinishOpen(ok) ==
    /\ opening /\ opening' = FALSE
    /\ IF ok /\ surface = "workspace" THEN Show("none") ELSE Keep
    /\ UNCHANGED <<workspace, prevented>>
Publish(hasWorkspace) ==
    /\ hasWorkspace # workspace /\ workspace' = hasWorkspace
    /\ IF surface = "workspace" THEN Keep ELSE Show("none")
    /\ UNCHANGED <<opening, prevented>>

Keys == {"k", "o", "r", "ignored"}
Next == \/ \E k \in Keys, b \in BOOLEAN : Key(k,b)
        \/ PickPath \/ Toolbar \/ RunButton \/ Edit
        \/ \E a \in {"open", "run", "other"}, b, c \in BOOLEAN : Choose(a,b,c)
        \/ \E s \in Owners \ {"none"}, b \in BOOLEAN : Close(s,b)
        \/ BeginOpen \/ \E b \in BOOLEAN : FinishOpen(b) \/ Publish(b)
Spec == Init /\ [][Next]_vars
NoOrphanOverlay == surface \in Owners /\ (~workspace => surface \in {"workspace", "none"})
OnlyFormEdited == edited => Form
GuardedClose == [][\A s \in Owners \ {"none"}, b \in BOOLEAN :
    Close(s,b) /\ (s # surface \/ (s = "palette" /\ b)) => UNCHANGED <<surface, edited>>]_vars
ShortcutsRespectOwner == [][\A k \in Keys, b \in BOOLEAN :
    Key(k,b) /\ (~workspace \/ k = "ignored" \/
                 (k \in {"k", "r"} /\ surface \in {"run", "workspace"}) \/
                 (k = "r" /\ ~b) \/ (k = "o" /\ (opening \/ surface = "workspace")))
    => UNCHANGED <<surface, edited>>]_vars
KeyDisposition == [][\A k \in Keys, b \in BOOLEAN : Key(k,b) =>
    /\ prevented' = (k = "o" \/ (workspace /\ k = "r") \/
                      (workspace /\ k = "k" /\ surface \in {"none", "palette"}))
    /\ focusInput' = (k = "k" /\ workspace /\ surface = "none")
    /\ (focusInput' => surface' = "palette")
    /\ (k = "k" /\ surface = "palette" => surface' = "none")
    /\ (k = "o" /\ workspace /\ ~opening /\ surface # "workspace" => surface' = "workspace")
    /\ (k = "r" /\ workspace /\ b /\ surface \in {"none", "palette"} => surface' = "run")]_vars
ExplicitPromptSurvivesLoss == [][Publish(FALSE) /\ surface = "workspace" =>
                                 UNCHANGED <<surface, edited>>]_vars
PaletteDispatch == [][\A a \in {"open", "run", "other"}, b, c \in BOOLEAN :
    Choose(a,b,c) =>
    /\ (surface' = "palette") = ~c
    /\ (surface' = "run") = (c /\ a = "run" /\ b)
    /\ (surface' = "workspace") = (c /\ a = "open" /\ ~opening)]_vars
OpenReplyRespectsOwner == [][\A b \in BOOLEAN :
    FinishOpen(b) /\ (~b \/ surface # "workspace") => UNCHANGED <<surface, edited>>]_vars
=============================================================================
