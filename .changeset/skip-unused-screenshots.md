---
"@agent-wechat/agent-server": patch
---

Skip the per-step screenshot unless the login QR screen needs it.

The FSM observe step captured a screenshot on every iteration and on every
health-monitor tick, but only the login QR state reads it. Gating the capture
on the same "Scan to log in" label that the QR state already checks removes
roughly 150 ms per step from every UI plan while leaving QR login untouched.
