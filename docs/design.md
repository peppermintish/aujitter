# AuJitter design

Selected agent skill: **UI UX Pro Max** from [nextlevelbuilder/ui-ux-pro-max-skill](https://github.com/nextlevelbuilder/ui-ux-pro-max-skill), together with [GPUI Kit's component and design skills](https://github.com/longbridge/gpui-kit/tree/main/skills). On 1 October 2026 the Pro Max repository had 132,071 GitHub stars. That is a transparent popularity proxy, not a claim that every design skill was surveyed.

The monitoring-dashboard design search suggested dark slate surfaces, restrained green status, readable typography, and a dense operational dashboard. The generated glass/blur direction was adapted to solid surfaces, system fonts and no continuous motion to suit a native desktop and a monitor kept open while gaming. GPUI Kit 0.7.0 is pinned; native controls, retained inputs, theme tokens, rem spacing, focus and keyboard actions follow its normative guidelines.

Three destinations stay in the same window: Network health, Incident history, Settings. Four headline measurements lead to a latency chart, confidence-qualified explanation, next steps and connection details. Status always has text; color is supplemental. Failure gaps are never plotted as zero milliseconds. The native chart shows the latest continuous run of timed replies; the web chart breaks lines at missing replies or observation gaps and includes a recent-observations table.

Gaming mode, pause and export are immediately available. Native Ctrl/Cmd+G changes gaming mode, Ctrl/Cmd+P pauses, Ctrl/Cmd+E exports, and Ctrl/Cmd+Q closes the UI. Inputs use the framework's native focus system. The browser has visible focus rings, semantic buttons, labelled inputs, status announcements, light/dark themes, overflow-safe tables and layouts down to a 375 px viewport.

Monitoring lives outside the desktop window. A bounded worker channel sends commands; the native render function performs no network or storage work. The desktop polls service state every five seconds, and a lightweight GPUI task updates only when the worker revision changes. The browser polls only while its tab is visible. Neither interface performs speed tests or assumes that an unknown WAN type is cellular.
