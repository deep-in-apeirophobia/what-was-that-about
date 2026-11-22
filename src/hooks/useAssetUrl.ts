import { useEffect, useState } from "react";
import { ASSET_NEEDS_BLOB, mediaUrl } from "../lib/platform";

/** URL an <img> can show for a file on disk: the asset URL, or on Linux a blob URL made
 *  from it (revoked when the component unmounts or the path changes). */
export function useAssetUrl(path: string | null | undefined): string | null {
  const direct = mediaUrl(path);
  const needsBlob = !!direct && ASSET_NEEDS_BLOB && !/^(blob:|data:)/i.test(direct);
  const [blobUrl, setBlobUrl] = useState<string | null>(null);

  useEffect(() => {
    if (!needsBlob || !direct) return;
    let url: string | null = null;
    let cancelled = false;
    fetch(direct)
      .then((r) => (r.ok ? r.blob() : Promise.reject(new Error(`HTTP ${r.status}`))))
      .then((b) => {
        if (cancelled) return;
        url = URL.createObjectURL(b);
        setBlobUrl(url);
      })
      .catch((e) => console.warn("couldn't load", path, e));
    return () => {
      cancelled = true;
      if (url) URL.revokeObjectURL(url);
      setBlobUrl(null);
    };
  }, [direct, needsBlob, path]);

  return needsBlob ? blobUrl : direct;
}
