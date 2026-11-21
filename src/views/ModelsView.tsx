import { useEffect } from "react";
import { Cpu, Folder, Gpu, HardDrive, MemoryStick, RefreshCw } from "lucide-react";
import { formatBytes } from "../lib/format";
import { useModels } from "../store/models";
import { useSettings } from "../store/settings";
import { Button } from "../components/ui/Button";
import { Badge, Spinner } from "../components/ui/misc";
import { ModelCard } from "../components/ModelCard";

function SystemPanel() {
  const system = useSettings((s) => s.system);
  const loadSystem = useSettings((s) => s.loadSystem);
  if (!system) {
    return (
      <div className="flex justify-center py-6">
        <Spinner />
      </div>
    );
  }
  const osName = { windows: "Windows", linux: "Linux", macos: "macOS" }[system.os] ?? system.os;
  return (
    <section className="rounded-xl border border-line bg-surface p-5 shadow-card">
      <div className="mb-3 flex items-center justify-between">
        <h2 className="text-[14px] font-semibold">This computer</h2>
        <Button size="sm" variant="ghost" icon={<RefreshCw className="size-3" />} onClick={() => void loadSystem()}>
          Refresh
        </Button>
      </div>
      <dl className="grid gap-x-6 gap-y-3 text-[13px] sm:grid-cols-2">
        <div className="flex items-start gap-2.5">
          <HardDrive className="mt-0.5 size-4 text-faint" />
          <div>
            <dt className="text-[12px] text-faint">System</dt>
            <dd>
              {osName} · {system.arch}
            </dd>
          </div>
        </div>
        <div className="flex items-start gap-2.5">
          <Cpu className="mt-0.5 size-4 text-faint" />
          <div>
            <dt className="text-[12px] text-faint">CPU</dt>
            <dd>{system.cpuCores} cores</dd>
          </div>
        </div>
        <div className="flex items-start gap-2.5">
          <MemoryStick className="mt-0.5 size-4 text-faint" />
          <div>
            <dt className="text-[12px] text-faint">Memory</dt>
            <dd>{formatBytes(system.totalMemoryBytes)}</dd>
          </div>
        </div>
        <div className="flex items-start gap-2.5">
          <Gpu className="mt-0.5 size-4 text-faint" />
          <div className="min-w-0">
            <dt className="text-[12px] text-faint">GPU acceleration</dt>
            <dd className="flex flex-wrap items-center gap-1.5">
              <span className="capitalize">{system.gpuBackend === "none" ? "Not compiled in" : system.gpuBackend}</span>
              {system.gpuBackend !== "none" &&
                (system.gpuAvailable ? <Badge tone="ok">Available</Badge> : <Badge tone="warn">Unavailable</Badge>)}
            </dd>
            {system.gpuDevices.length > 0 ? (
              <ul className="mt-1 space-y-0.5 text-[12px] text-muted">
                {system.gpuDevices.map((d) => (
                  <li key={d.name}>
                    {d.name}
                    {d.memoryBytes ? ` · ${formatBytes(d.memoryBytes)}` : ""}
                    {d.description ? <span className="text-faint"> — {d.description}</span> : null}
                  </li>
                ))}
              </ul>
            ) : (
              system.gpuBackend !== "none" &&
              !system.gpuAvailable && (
                <p className="mt-1 text-[12px] text-muted">
                  No usable GPU was found (is the {system.gpuBackend === "vulkan" ? "Vulkan loader" : "GPU driver"} installed?). Transcription
                  will use the CPU.
                </p>
              )
            )}
          </div>
        </div>
        <div className="flex items-start gap-2.5 sm:col-span-2">
          <Folder className="mt-0.5 size-4 text-faint" />
          <div className="min-w-0">
            <dt className="text-[12px] text-faint">Data folder</dt>
            <dd className="truncate font-mono text-[12px]" title={system.dataDir}>
              {system.dataDir}
            </dd>
          </div>
        </div>
      </dl>
      <div className="mt-4 border-t border-line pt-3 text-[11.5px] text-faint">
        Version {system.appVersion} · Engines: Whisper {system.engines.whisper ? "✓" : "✗"} · Shenava {system.engines.shenava ? "✓" : "✗"}
      </div>
    </section>
  );
}

export function ModelsView() {
  const models = useModels((s) => s.models);
  const loaded = useModels((s) => s.loaded);
  const refresh = useModels((s) => s.refresh);
  const system = useSettings((s) => s.system);
  const recommendedEngine = system ? (system.gpuAvailable ? "whisper" : "shenava") : null;

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return (
    <div className="flex h-full flex-col">
      <header className="border-b border-line px-6 py-3.5">
        <h1 className="text-[17px] font-semibold">Speech models</h1>
        <p className="text-[12.5px] text-muted">Models run locally — audio never leaves this computer. Install one or both.</p>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto max-w-4xl space-y-4 p-6">
          {!loaded ? (
            <div className="flex justify-center py-10">
              <Spinner />
            </div>
          ) : (
            models.map((m) => (
              <ModelCard
                key={m.id}
                model={m}
                recommended={m.engine === recommendedEngine && !models.some((x) => x.status === "ready")}
              />
            ))
          )}
          <SystemPanel />
        </div>
      </div>
    </div>
  );
}
