/** What the agent card is given: plain values only (they cross into the widget). */
export interface AgentCardProps {
  summary: string;
  needsYou: number;
  working: number;
  rows: {
    key: string;
    title: string;
    place: string;
    /** needs_input | failed | review | working */
    state: string;
    label: string;
    since: number;
  }[];
}
