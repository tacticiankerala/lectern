// Short messages at the bottom of the window.

const DEFAULT_MS = 3500;

export class Toasts {
  constructor(private readonly host: HTMLElement) {}

  show(message: string, ms = DEFAULT_MS): void {
    const toast = document.createElement("div");
    toast.className = "toast";
    toast.setAttribute("role", "status");
    toast.textContent = message;
    this.host.append(toast);
    window.setTimeout(() => {
      toast.remove();
    }, ms);
  }
}
