<script setup lang="ts">
import mobileSettingView from '@/assets/images/mobile-setting-view.jpeg'
import mobileStreamDemo from '@/assets/images/mobile-stream-demo.gif'
import pcSystemTray from '@/assets/images/pc-system-tray.png'
import SectionHeading from '@/components/shared/SectionHeading.vue'
import DemoShot from './DemoShot.vue'

interface Step {
  title: string
  body: string
}

const steps: Step[] = [
  {
    title: 'Install both apps',
    body: 'The tray app on the PC, the APK on the phone.',
  },
  {
    title: 'Connect them on the same network',
    body: 'Over Wi-Fi or USB tethering.',
  },
  {
    title: 'Connect & approve the pairing req',
    body: 'Both screens should show the same 6 digits.',
  },
  {
    title: 'Pick a source',
    body: 'The whole desktop or one app.',
  },
]
</script>

<template>
  <section
    id="demo"
    class="flex flex-col items-center justify-center gap-y-10 px-4 py-20 sm:px-6 sm:py-24 lg:px-10"
  >
    <SectionHeading
      eyebrow="Demo"
      title="Adaptive low-latency audio buffer"
      subtitle="The phone shows its buffer, RTT, and jitter live."
    />

    <div
      class="grid w-full max-w-6xl grid-cols-1 items-stretch gap-4 sm:gap-5 md:grid-cols-2 lg:grid-cols-3"
    >
      <DemoShot
        :src="mobileStreamDemo"
        :width="576"
        :height="1296"
        tall
        label="Phone / playing"
        alt="The Gemacast player on an Android phone showing buffer and jitter while streaming desktop audio"
        caption="Buffer and jitter, the PC, and the audio source."
      />

      <DemoShot
        :src="mobileSettingView"
        :width="575"
        :height="1280"
        tall
        label="Phone / settings"
        alt="The Gemacast settings screen on Android with buffer preset, bitrate, gain and transport mode controls"
        caption="Buffer preset, bitrate, gain, and the Wi-Fi / USB connection selector."
      />

      <div class="flex flex-col gap-4 sm:gap-5 md:col-span-2 md:flex-row lg:col-span-1 lg:flex-col">
        <DemoShot
          class="md:flex-1 lg:flex-none"
          :src="pcSystemTray"
          :width="861"
          :height="421"
          label="PC / system tray"
          alt="The Gemacast tray menu on Windows listing a connected phone, stream controls and update options"
          caption="Connected phones, start and stop stream, updates, launch on startup."
        />

        <ol
          class="offset-shadow flex grow flex-col divide-y divide-border border border-primary bg-card md:flex-1 lg:flex-none"
        >
          <li
            v-for="(step, index) in steps"
            :key="step.title"
            class="flex grow items-start gap-x-3 p-4"
          >
            <span
              class="font-mono flex size-6 shrink-0 items-center justify-center border border-primary bg-secondary text-xs font-bold text-primary"
            >
              {{ index + 1 }}
            </span>
            <div class="flex flex-col gap-y-1">
              <h3 class="font-mono text-sm font-bold text-primary">{{ step.title }}</h3>
              <p class="font-mono text-xs leading-relaxed text-secondary-foreground">
                {{ step.body }}
              </p>
            </div>
          </li>
        </ol>
      </div>
    </div>
  </section>
</template>
