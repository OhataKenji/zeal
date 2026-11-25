------------------------- MODULE TimelineInteraction -------------------------
EXTENDS Integers, FiniteSets
CONSTANTS Pointers, Lanes, None, Pan
Indices == 0..1
Events == Lanes \X Indices
VARIABLES drag, moved, selected, pending, effect
vars == <<drag, moved, selected, pending, effect>>
Init == /\ drag = None /\ moved = FALSE /\ selected = None /\ pending = {} /\ effect = None

\* drag is the handler's single saved pointer id (not a set of browser
\* captures). A second qualifying down replaces it even before the first up.
Down(p, primaryInPlot) ==
    /\ IF primaryInPlot
       THEN /\ drag' = p /\ moved' = FALSE /\ effect' = <<"capture", p>>
       ELSE /\ UNCHANGED <<drag, moved>> /\ effect' = None
    /\ UNCHANGED <<selected, pending>>
Move(p, pastThreshold) ==
    /\ moved' = (moved \/ (p = drag /\ pastThreshold))
    /\ effect' = IF p = drag /\ moved' THEN Pan ELSE None
    /\ UNCHANGED <<drag, selected, pending>>
Up(p, hit, hasCapture) ==
    /\ IF drag = p
       THEN /\ drag' = None /\ moved' = FALSE
            /\ effect' = IF hasCapture THEN <<"release", p>> ELSE None
            /\ selected' = IF ~moved /\ hit # None THEN hit ELSE selected
       ELSE /\ UNCHANGED <<drag, moved, selected>> /\ effect' = None
    /\ UNCHANGED pending
\* cancel and lostpointercapture clear drag without checking the event id.
Cancel == /\ drag' = None /\ moved' = FALSE /\ effect' = None /\ UNCHANGED <<selected, pending>>

Adjacent(e,d) == <<e[1], IF e[2]+d \in Indices THEN e[2]+d ELSE e[2]>>
Arrow(d, canvasKey) ==
    /\ IF canvasKey /\ selected # None
       THEN /\ selected' = Adjacent(selected,d)
            /\ pending' = pending \cup {selected'}
            /\ Cardinality(pending') <= 2
       ELSE UNCHANGED <<selected, pending>>
    /\ effect' = None /\ UNCHANGED <<drag, moved>>
Escape == /\ selected' = None /\ effect' = None /\ UNCHANGED <<drag, moved, pending>>

\* Overapproximate tick order. Owner is the pair (tid,index), not the
\* generation of the key press: returning to the same pair may reveal it.
Tick(e, canReveal) ==
    /\ e \in pending /\ pending' = pending \ {e}
    /\ effect' = IF selected = e /\ canReveal THEN <<"reveal", e>> ELSE None
    /\ UNCHANGED <<drag, moved, selected>>
Next == \/ \E p \in Pointers, b \in BOOLEAN : Down(p,b) \/ Move(p,b)
        \/ \E p \in Pointers, hit \in Events \cup {None}, b \in BOOLEAN : Up(p,hit,b)
        \/ Cancel \/ Escape
        \/ \E d \in {-1,1}, b \in BOOLEAN : Arrow(d,b)
        \/ \E e \in pending, b \in BOOLEAN : Tick(e,b)
Spec == Init /\ [][Next]_vars
WellFormed == /\ drag \in Pointers \cup {None} /\ (moved => drag # None)
              /\ selected \in Events \cup {None} /\ pending \subseteq Events
OwnedReveal == effect \in {None, Pan} \cup ({"capture","release"} \X Pointers)
               \/ (selected # None /\ effect = <<"reveal", selected>>)
CaptureOnPlot == [][\A p \in Pointers, b \in BOOLEAN : Down(p,b) =>
    IF b THEN drag' = p /\ ~moved' /\ effect' = <<"capture",p>>
    ELSE UNCHANGED <<drag,moved>> /\ effect' = None]_vars
PanOnlyAfterDrag == [][\A p \in Pointers, b \in BOOLEAN : Move(p,b) =>
    /\ (effect' = Pan) = (drag = p /\ (moved \/ b))
    /\ (moved => moved')
    /\ (drag # p => UNCHANGED moved)]_vars
ReleaseOrIgnore == [][\A p \in Pointers, h \in Events \cup {None}, b \in BOOLEAN : Up(p,h,b) =>
    /\ (drag = p => drag' = None /\ effect' = IF b THEN <<"release",p>> ELSE None)
    /\ (drag # p => UNCHANGED <<drag,moved>> /\ effect' = None)
    /\ (drag # p \/ moved \/ h = None => selected' = selected)
    /\ (drag = p /\ ~moved /\ h # None => selected' = h)]_vars
KeyboardSelection == [][\A d \in {-1,1}, b \in BOOLEAN : Arrow(d,b) =>
    IF ~b \/ selected = None THEN UNCHANGED <<selected,pending>>
    ELSE /\ selected'[1] = selected[1]
         /\ selected'[2] = IF selected[2]+d < 0 THEN 0
                           ELSE IF selected[2]+d > 1 THEN 1 ELSE selected[2]+d
         /\ selected' \in pending']_vars
CurrentTickReveals == [][\A e \in pending : Tick(e,TRUE) /\ selected = e =>
                                             effect' = <<"reveal",e>>]_vars
=============================================================================
