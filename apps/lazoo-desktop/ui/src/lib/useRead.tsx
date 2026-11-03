import { useCallback, useEffect, useState } from "react";

import { asFailure } from "./bridge";
import { useBridge } from "./context";
import type { RpcFailure } from "./protocol";

/** Read one daemon method; the result is shown as it is, never invented. */
export function useRead<T>(
  method: string,
  params: unknown = {},
  deps: unknown[] = [],
) {
  const bridge = useBridge();
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<RpcFailure | null>(null);
  const [version, setVersion] = useState(0);
  const reload = useCallback(() => setVersion((value) => value + 1), []);
  useEffect(() => {
    let cancelled = false;
    setError(null);
    bridge
      .call<T>(method, params)
      .then((value) => {
        if (!cancelled) setData(value);
      })
      .catch((failure: unknown) => {
        if (!cancelled) setError(asFailure(failure));
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [method, version, ...deps]);
  return { data, error, reload };
}

export function Failure({ error }: { error: RpcFailure | null }) {
  if (!error) return null;
  return (
    <p className="meta" role="alert">
      {error.code}: {error.message}
    </p>
  );
}
