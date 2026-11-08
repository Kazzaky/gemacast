<script setup lang="ts">
import type { Component } from 'vue'
import {
  ActivityIcon,
  CableIcon,
  LayersIcon,
  ShieldCheckIcon,
  SlidersHorizontalIcon,
  TabletSmartphoneIcon,
} from '@lucide/vue'
import SectionHeading from '@/components/shared/SectionHeading.vue'
import FeatureCard from './FeatureCard.vue'

interface Feature {
  icon: Component
  title: string
  body: string
  tag: string
}

const features: Feature[] = [
  {
    icon: TabletSmartphoneIcon,
    title: 'Support multiple phones',
    body: 'Connect several phones at once. Each one picks its own aduio source and bitrate quality.',
    tag: 'One stream per phone',
  },
  {
    icon: CableIcon,
    title: 'Two ways to connect',
    body: 'Wi-Fi over your LAN or USB tethering. Switch between them from the phone.',
    tag: 'Wi-Fi / USB',
  },
  {
    icon: ActivityIcon,
    title: 'Adaptive & customizable buffer ',
    body: 'The buffer sizes itself based on the network condition. Buffer size is also customizable.',
    tag: 'Sized dynamically',
  },
  {
    icon: ShieldCheckIcon,
    title: 'Paired with a code',
    body: 'The first connection shows the same 6 digits on both screens. Approve it once on the PC and the phone stays paired.',
    tag: 'TLS with a pinned certificate',
  },
  {
    icon: LayersIcon,
    title: 'Whole desktop or one app',
    body: 'Send everything the PC plays, or just one app. Change it mid-stream without reconnecting.',
    tag: 'Per-app capture',
  },
  {
    icon: SlidersHorizontalIcon,
    title: 'Customizable audio quality',
    body: 'Nine Opus bitrates from 10 to 512 kbps, a custom value, or uncompressed PCM. Change it while streaming.',
    tag: '10 to 512 kbps or raw',
  },
]

const specs: string[] = ['48 kHz stereo', '10 ms frames', 'Opus or raw PCM']
</script>

<template>
  <section
    id="features"
    class="flex flex-col items-center justify-center gap-y-10 px-4 py-20 sm:px-6 sm:py-24 lg:px-10"
  >
    <SectionHeading
      eyebrow="Features"
      title="What Gemacast does"
      subtitle="All available at current release."
    />

    <ul class="grid w-full max-w-6xl grid-cols-1 gap-4 sm:gap-5 md:grid-cols-2 lg:grid-cols-3">
      <FeatureCard
        v-for="feature in features"
        :key="feature.title"
        :icon="feature.icon"
        :title="feature.title"
        :body="feature.body"
        :tag="feature.tag"
      />
    </ul>

    <ul
      class="flex w-full max-w-6xl flex-col divide-y divide-border border border-primary bg-secondary sm:flex-row sm:divide-x sm:divide-y-0"
    >
      <li
        v-for="spec in specs"
        :key="spec"
        class="font-mono flex-1 px-4 py-3 text-center text-xs font-bold text-secondary-foreground sm:text-sm"
      >
        {{ spec }}
      </li>
    </ul>
  </section>
</template>
