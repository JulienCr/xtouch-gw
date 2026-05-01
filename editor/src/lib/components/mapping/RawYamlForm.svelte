<script lang="ts">
  export let mapping: Record<string, unknown> = {};

  let text = JSON.stringify(mapping ?? {}, null, 2);
  let invalid = false;
  let lastSerialized = text;

  // Re-sync the textarea when mapping is replaced from outside (e.g. kind switch),
  // but not when our own input drove the change (lastSerialized matches).
  $: {
    const ext = JSON.stringify(mapping ?? {}, null, 2);
    if (ext !== lastSerialized) {
      text = ext;
      lastSerialized = ext;
      invalid = false;
    }
  }

  function onInput(e: Event): void {
    const v = (e.currentTarget as HTMLTextAreaElement).value;
    text = v;
    try {
      const parsed = JSON.parse(v);
      if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
        invalid = false;
        lastSerialized = v;
        mapping = parsed as Record<string, unknown>;
      } else {
        invalid = true;
      }
    } catch {
      invalid = true;
    }
  }
</script>

<div class="space-y-2">
  <div class="flex items-center justify-between">
    <div class="text-xs text-slate-700 dark:text-slate-400">Mapping JSON</div>
    {#if invalid}
      <span class="text-xs px-1.5 py-0.5 rounded bg-rose-100 text-rose-700 border border-rose-300 dark:bg-rose-900/40 dark:text-rose-300 dark:border-rose-700">invalid JSON</span>
    {/if}
  </div>
  <textarea
    class="w-full font-mono text-xs bg-white border text-slate-900 dark:bg-slate-900 dark:text-slate-100 rounded p-2 h-56"
    class:border-rose-400={invalid}
    class:dark:border-rose-700={invalid}
    class:border-slate-300={!invalid}
    class:dark:border-slate-700={!invalid}
    value={text}
    spellcheck="false"
    on:input={onInput}
  ></textarea>
  <div class="text-xs text-slate-500 dark:text-slate-400 italic">
    Edits apply only when JSON is valid. Use this for mapping shapes the structured forms don't cover.
  </div>
</div>
