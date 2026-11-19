/** Pure keyboard navigation and scroll-intent rules; no DOM/native dependencies. */
export function radioKeyIndex(key: string, index: number, count: number): number | null {
  if (count < 1 || index < 0 || index >= count) return null;
  switch (key) {
    case " ":
    case "Spacebar":
    case "Enter": return index;
    case "ArrowRight":
    case "ArrowDown": return (index + 1) % count;
    case "ArrowLeft":
    case "ArrowUp": return (index + count - 1) % count;
    case "Home": return 0;
    case "End": return count - 1;
    default: return null;
  }
}

export function scrollKeyDirection(key: string, shiftKey = false): -1 | 1 | null {
  switch (key) {
    case "ArrowUp":
    case "PageUp":
    case "Home": return -1;
    case "ArrowDown":
    case "PageDown":
    case "End": return 1;
    case " ":
    case "Spacebar": return shiftKey ? -1 : 1;
    default: return null;
  }
}

/** Only real input can pause follow; layout/programmatic scrolling cannot. */
export class ScrollFollowing {
  private following = true;
  private interacting = false;

  get canFollow(): boolean { return this.following && !this.interacting; }
  get isInteracting(): boolean { return this.interacting; }

  beginInteraction(): void { this.interacting = true; }

  towardHistory(offset: number): void {
    if (offset > 0) this.following = false;
  }

  scrolled(atBottom: boolean): void {
    // Resuming at the bottom also handles wheel/keyboard input, which has no
    // native drag lifecycle. Output growth must not pause an active follower.
    if (this.interacting || !this.following) this.following = atBottom;
  }

  endInteraction(atBottom: boolean): void {
    this.following = atBottom;
    this.interacting = false;
  }
}
