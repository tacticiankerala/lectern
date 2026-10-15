// Short messages at the bottom of the window.

const DEFAULT_MS = 3500;

export class Toasts {
  constructor(private readonly host: HTMLElement) {}

  /** Shows `message`, with `detail` on a line of its own below it when given. */
  show(message: string, detail?: string, ms = DEFAULT_MS): void {
    const toast = document.createElement("div");
    toast.className = "toast";
    toast.setAttribute("role", "status");
    toast.textContent = message;
    if (detail !== undefined) {
      const line = document.createElement("span");
      line.className = "toast-detail";
      line.textContent = detail;
      toast.append(line);
    }
    this.host.append(toast);
    window.setTimeout(() => {
      toast.remove();
    }, ms);
  }
}
