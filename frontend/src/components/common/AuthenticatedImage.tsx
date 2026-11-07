import { useCallback, useEffect, useState } from "react";

import { fetchFileBlob, fileRawUrl } from "../../api/client";
import "./AuthenticatedImage.css";

export function AuthenticatedImage({
  repo,
  workspaceId,
  path,
  alt,
  className,
}: {
  repo: string;
  workspaceId?: string;
  path: string;
  alt: string;
  className?: string;
}) {
  const [src, setSrc] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    let objectUrl: string | null = null;
    setSrc(null);
    setError(null);
    void (async () => {
      try {
        const blob = await fetchFileBlob(
          fileRawUrl({ repo, workspaceId }, path),
        );
        if (cancelled) return;
        objectUrl = URL.createObjectURL(blob);
        setSrc(objectUrl);
      } catch (err) {
        if (!cancelled)
          setError(err instanceof Error ? err.message : "image fetch failed");
      }
    })();
    return () => {
      cancelled = true;
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [repo, path, workspaceId]);

  const onDecodeError = useCallback(
    () => setError("The image could not be decoded."),
    [],
  );
  if (error)
    return (
      <span className="authenticated-image__status" role="status">
        Image preview unavailable: {error}
      </span>
    );
  if (!src)
    return <span className="authenticated-image__status">Loading image…</span>;
  return (
    // eslint-disable-next-line jsx-a11y/no-noninteractive-element-interactions -- onError handles passive image decoding failures.
    <img
      src={src}
      alt={alt}
      className={className}
      loading="lazy"
      onError={onDecodeError}
    />
  );
}
