import { useState } from "react";
import { Link2 } from "lucide-react";
import { useApp } from "../store/app";
import { useLibrary } from "../store/library";
import { Button } from "./ui/Button";
import { Dialog } from "./ui/Dialog";
import { inputClass } from "./ui/misc";

/** Import an Adobe Connect recording by pasting its link. */
export function ConnectLinkDialog() {
  const open = useApp((s) => s.linkDialogOpen);
  const close = () => useApp.getState().setLinkDialogOpen(false);
  const [url, setUrl] = useState("");
  const [busy, setBusy] = useState(false);
  const valid = /^https?:\/\/[^/\s]+\/[^/\s?#]+/i.test(url.trim());

  const submit = async () => {
    if (!valid || busy) return;
    setBusy(true);
    const rec = await useLibrary.getState().importLink(url.trim());
    setBusy(false);
    if (rec) {
      setUrl("");
      close();
    }
  };

  return (
    <Dialog
      open={open}
      onClose={close}
      title="Import an Adobe Connect recording"
      description="Paste the recording's link. The app downloads its source archive, mixes every speaker's audio onto one timeline and transcribes it."
      footer={
        <>
          <Button onClick={close}>Cancel</Button>
          <Button variant="primary" icon={<Link2 className="size-3.5" />} disabled={!valid || busy} onClick={() => void submit()}>
            {busy ? "Starting…" : "Import"}
          </Button>
        </>
      }
    >
      <form
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
        className="space-y-3"
      >
        <input
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          placeholder="https://connect.example.edu/p1a2b3c4d5/?session=…"
          aria-label="Adobe Connect recording link"
          dir="ltr"
          spellCheck={false}
          className={inputClass}
        />
        <ul className="list-disc space-y-1.5 pl-5 text-[12.5px] leading-relaxed text-muted">
          <li>
            If the recording needs a login, open it in your browser first and copy the address from there; it then
            contains <code className="rounded bg-sunken px-1">?session=…</code>, which lets the app download it as you.
          </li>
          <li>
            You can also download the archive yourself: add{" "}
            <code className="rounded bg-sunken px-1">/output/download.zip?download=zip</code> to the recording link, then
            drop the ZIP (or the extracted folder) onto the app.
          </li>
        </ul>
      </form>
    </Dialog>
  );
}
