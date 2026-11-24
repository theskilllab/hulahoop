import { useCallback, useEffect, useRef, useState } from "react";

/**
 * One transient line at the bottom of the work area: "Saved", "BAL-4 → In review",
 * or the server's reason for refusing a move. Returns the node to render and a
 * function to show a message.
 */
export function useToast(duration = 2600) {
  const [message, setMessage] = useState<{ text: string; bad: boolean } | null>(null);
  const timer = useRef<number | undefined>(undefined);

  const show = useCallback(
    (text: string, bad = false) => {
      window.clearTimeout(timer.current);
      setMessage({ text, bad });
      timer.current = window.setTimeout(() => setMessage(null), bad ? duration * 2 : duration);
    },
    [duration],
  );

  useEffect(() => () => window.clearTimeout(timer.current), []);

  const node = message ? (
    <div className={message.bad ? "toast bad" : "toast"} role={message.bad ? "alert" : "status"}>
      {message.text}
    </div>
  ) : null;

  return [node, show] as const;
}
