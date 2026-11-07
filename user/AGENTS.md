# Application navigation headers

Headers, including custom headers and extension rows, must be passed to
ScarletUI navigation. Do not stack a second page header around navigation.
The root header layout describes its body outline: do not add outer padding,
shadow clearance, or navigation-width compensation. Navigation owns outer
margins, inter-surface gaps, and the content safe-area reservation.
Custom appearance and internal padding are supported. Shadow overflow uses
`paint_outsets()`, not layout padding. ScrollView consumes the reservation;
fixed content uses `safe_area_padding` once. File pickers follow the same rule.
