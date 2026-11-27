// Viewer display preferences: stream visibility (hooks, thoughts, timestamps,
// agent telemetry, live diffs) and chroma palette. Stored in this browser's localStorage
// alongside appearance; every setting has a calm, low-noise default.

export const hookModes = ["off", "errors", "all"] as const;
export type HookMode = (typeof hookModes)[number];

export const thoughtModes = ["hidden", "collapsed", "expanded"] as const;
export type ThoughtMode = (typeof thoughtModes)[number];

export const timestampModes = ["hover", "auto", "off"] as const;
export type TimestampMode = (typeof timestampModes)[number];

export const telemetryModes = ["minimal", "compact", "detailed"] as const;
export type TelemetryMode = (typeof telemetryModes)[number];

export const palettes = ["sumi", "violet", "mono"] as const;
export type Palette = (typeof palettes)[number];

export const presets = ["focus", "standard", "debug"] as const;
export type ViewPreset = (typeof presets)[number];

export type StoredPreferences = {
  hooks: HookMode;
  thoughts: ThoughtMode;
  timestamps: TimestampMode;
  telemetry: TelemetryMode;
  liveDiff: boolean;
  palette: Palette;
};

export const presetValues: Record<ViewPreset, Omit<StoredPreferences, "liveDiff" | "palette">> = {
  focus: {
    hooks: "errors",
    thoughts: "collapsed",
    timestamps: "hover",
    telemetry: "compact",
  },
  standard: {
    hooks: "errors",
    thoughts: "collapsed",
    timestamps: "auto",
    telemetry: "detailed",
  },
  debug: {
    hooks: "all",
    thoughts: "collapsed",
    timestamps: "auto",
    telemetry: "detailed",
  },
};

const storageKey = "mekugi.preferences";

function pick<T extends string>(value: unknown, allowed: readonly T[], fallback: T): T {
  return typeof value === "string" && (allowed as readonly string[]).includes(value)
    ? (value as T)
    : fallback;
}

class Preferences {
  hooks = $state<HookMode>("errors");
  thoughts = $state<ThoughtMode>("collapsed");
  timestamps = $state<TimestampMode>("hover");
  telemetry = $state<TelemetryMode>("compact");
  liveDiff = $state(true);
  palette = $state<Palette>("sumi");
  storageError = $state<string>();

  constructor() {
    try {
      const raw = localStorage.getItem(storageKey);
      if (raw) {
        const parsed = JSON.parse(raw) as Partial<StoredPreferences>;
        this.hooks = pick(parsed.hooks, hookModes, "errors");
        this.thoughts = pick(parsed.thoughts, thoughtModes, "collapsed");
        this.timestamps = pick(parsed.timestamps, timestampModes, "hover");
        this.telemetry = pick(parsed.telemetry, telemetryModes, "compact");
        this.liveDiff = parsed.liveDiff !== false;
        this.palette = pick(parsed.palette, palettes, "sumi");
      }
    } catch {
      // Unreadable or malformed storage leaves calm defaults.
    }
  }

  get preset(): ViewPreset | "custom" {
    for (const name of presets) {
      const target = presetValues[name];
      if (
        this.hooks === target.hooks &&
        this.thoughts === target.thoughts &&
        this.timestamps === target.timestamps &&
        this.telemetry === target.telemetry
      ) {
        return name;
      }
    }
    return "custom";
  }

  private persist(): void {
    try {
      const payload: StoredPreferences = {
        hooks: this.hooks,
        thoughts: this.thoughts,
        timestamps: this.timestamps,
        telemetry: this.telemetry,
        liveDiff: this.liveDiff,
        palette: this.palette,
      };
      localStorage.setItem(storageKey, JSON.stringify(payload));
      this.storageError = undefined;
    } catch {
      this.storageError =
        "Display settings could not be saved and apply until reload. Check this site's browser storage settings.";
    }
  }

  apply(): void {
    document.documentElement.dataset.palette = this.palette;
  }

  setHooks(mode: HookMode): void {
    this.hooks = mode;
    this.persist();
  }

  setThoughts(mode: ThoughtMode): void {
    this.thoughts = mode;
    this.persist();
  }

  setTimestamps(mode: TimestampMode): void {
    this.timestamps = mode;
    this.persist();
  }

  setTelemetry(mode: TelemetryMode): void {
    this.telemetry = mode;
    this.persist();
  }

  setLiveDiff(enabled: boolean): void {
    this.liveDiff = enabled;
    this.persist();
  }

  setPalette(palette: Palette): void {
    this.palette = palette;
    this.apply();
    this.persist();
  }

  applyPreset(preset: ViewPreset): void {
    const target = presetValues[preset];
    this.hooks = target.hooks;
    this.thoughts = target.thoughts;
    this.timestamps = target.timestamps;
    this.telemetry = target.telemetry;
    this.persist();
  }

  toggleHooks(): void {
    this.setHooks(this.hooks === "all" ? "errors" : "all");
  }

  toggleDebug(): void {
    this.applyPreset(this.preset === "debug" ? "focus" : "debug");
  }
}

export const preferences = new Preferences();
