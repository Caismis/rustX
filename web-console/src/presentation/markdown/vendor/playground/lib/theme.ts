import { useSyncExternalStore } from "react";

const attribute = "data-ds-dark-theme";
function subscribe(changed: () => void) {
  const observer = new MutationObserver(changed);
  observer.observe(document.body, { attributeFilter: [attribute] });
  return () => observer.disconnect();
}

function read() {
  return document.body.hasAttribute(attribute) ? "dark" : "light";
}

export function useTheme() {
  return useSyncExternalStore(subscribe, read, () => "light");
}
