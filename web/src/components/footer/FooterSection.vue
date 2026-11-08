<script setup lang="ts">
import { CodeIcon, DownloadIcon, ExternalLinkIcon, MailIcon, ScaleIcon } from "@lucide/vue";
import { links } from "@/core/links";

interface FooterLink {
  label: string;
  href: string;
  external: boolean;
}

const projectLinks: FooterLink[] = [
  { label: "Source on GitHub", href: links.repo, external: true },
  { label: "Latest release", href: links.releases, external: true },
  { label: "Report an issue", href: links.issues, external: true },
];

const pageLinks: FooterLink[] = [
  { label: "Features", href: "#features", external: false },
  { label: "Demo", href: "#demo", external: false },
  { label: "Download", href: "#download", external: false },
  { label: "FAQ", href: "#faq", external: false },
];
</script>

<template>
  <footer class="bg-foreground text-accent">
    <div
      class="mx-auto flex max-w-6xl flex-col gap-8 px-4 py-10 sm:px-6 sm:py-12 md:flex-row md:justify-between lg:px-10"
    >
      <div class="flex flex-col gap-y-3 md:max-w-md">
        <div class="flex items-center gap-x-2">
          <img
            src="../../assets/images/logo.svg"
            width="36"
            height="36"
            alt="Gemacast"
            class="size-9"
          />
          <span class="font-[Black_Ops_One] text-lg text-secondary">Gemacast</span>
        </div>
        <p class="font-mono text-xs leading-relaxed text-accent sm:text-sm">
          Streams your PC's audio to your phone over Wi-Fi, USB, or a cable, with tens of
          milliseconds of delay.
        </p>
        <a
          :href="links.contact"
          class="font-mono flex w-fit items-center gap-x-2 text-xs text-accent underline decoration-accent/40 underline-offset-4 transition-colors sm:text-sm sm:hover:text-secondary"
        >
          <MailIcon class="size-4 shrink-0" />
          <span>echa.apriliyanto.dev@gmail.com</span>
        </a>
      </div>

      <nav class="grid grid-cols-2 gap-8 sm:gap-12">
        <div class="flex flex-col gap-y-3">
          <h3 class="font-mono text-xs font-bold uppercase tracking-[0.2em] text-secondary">
            Project
          </h3>
          <ul class="flex flex-col gap-y-2">
            <li v-for="link in projectLinks" :key="link.label">
              <a
                :href="link.href"
                target="_blank"
                rel="noreferrer"
                class="font-mono flex items-center gap-x-1.5 text-xs text-accent transition-colors sm:text-sm sm:hover:text-secondary"
              >
                <span>{{ link.label }}</span>
                <ExternalLinkIcon class="size-3 shrink-0" />
              </a>
            </li>
          </ul>
        </div>

        <div class="flex flex-col gap-y-3">
          <h3 class="font-mono text-xs font-bold uppercase tracking-[0.2em] text-secondary">
            On this page
          </h3>
          <ul class="flex flex-col gap-y-2">
            <li v-for="link in pageLinks" :key="link.label">
              <a
                :href="link.href"
                class="font-mono text-xs text-accent transition-colors sm:text-sm sm:hover:text-secondary"
              >
                {{ link.label }}
              </a>
            </li>
          </ul>
        </div>
      </nav>
    </div>

    <div class="border-t border-accent/20">
      <div
        class="mx-auto flex max-w-6xl flex-col items-center gap-3 px-4 py-5 sm:flex-row sm:justify-between sm:px-6 lg:px-10"
      >
        <div class="flex flex-col text-xs gap-y-1">
          <span class="font-mono text-center text-accent"> Built by Echa Apriliyanto. </span>
          <div class="flex flex-row gap-x-1 items-center">
            <ScaleIcon class="size-4"/>
            <a href="https://github.com/Kazzaky/gemacast?tab=GPL-3.0-1-ov-file" target="_blank">GPL 3.0</a>
          </div>
        </div>
        <div class="flex items-center gap-x-2">
          <a
            :href="links.releases"
            target="_blank"
            rel="noreferrer"
            class="font-mono flex items-center gap-x-1.5 border border-accent/30 px-3 py-1.5 text-xs text-accent transition-colors sm:hover:border-accent sm:hover:bg-accent/10"
          >
            <DownloadIcon class="size-3.5" />
            <span>Releases</span>
          </a>
          <a
            :href="links.repo"
            target="_blank"
            rel="noreferrer"
            class="font-mono flex items-center gap-x-1.5 border border-accent/30 px-3 py-1.5 text-xs text-accent transition-colors sm:hover:border-accent sm:hover:bg-accent/10"
          >
            <CodeIcon class="size-3.5" />
            <span>Source</span>
          </a>
        </div>
      </div>
    </div>
  </footer>
</template>
