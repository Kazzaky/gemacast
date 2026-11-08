<script setup lang="ts">
import { useAccordion } from '@/composables/useAccordion'
import SectionHeading from '@/components/shared/SectionHeading.vue'
import FaqItem from './FaqItem.vue'

interface Question {
  id: string
  question: string
  answer: string
}

const questions: Question[] = [
  {
    id: 'discovery',
    question: 'The phone cannot find my PC',
    answer:
      'Both devices must be on the same reachable network. Guest Wi-Fi, VPNs, and router client isolation can hide the PC. Confirm that Gemacast is running and that UDP 23555-23556 and TCP 23559 are allowed through the PC firewall.',
  },
  {
    id: 'latency',
    question: 'What is the real end-to-end latency?',
    answer:
      "End-to-end latency includes PC capture and packetization, one-way transport, the phone's jitter buffer, decoding, and the phone's audio-output buffer. The phone reports round-trip time and jitter-buffer depth. RTT does not determine one-way delay unless the path is symmetric, and the app does not measure capture or hardware-output latency, so those metrics are not a complete end-to-end value.",
  },
  {
    id: 'dtim',
    question: 'The buffer grows past 200 ms when the screen turns off',
    answer:
      'Some Android devices change Wi-Fi scheduling when the screen turns off. If packets begin arriving in larger batches, the adaptive buffer grows in response. Keep Screen On avoids the screen-off state; USB tethering avoids the Wi-Fi path.',
  },
  {
    id: 'pairing',
    question: 'Why is there a pairing step?',
    answer:
      "Pairing decides which phones may control the PC, request its process list, and select what it captures. After approval, the phone stores the PC identity. Automatic reconnection depends on the app's Auto Reconnect setting.",
  },
  {
    id: 'code',
    question: 'What is the six-digit code for?',
    answer:
      'Both devices independently derive the code from the authenticated pairing exchange. Matching codes provide a human check against a man-in-the-middle connection. Compare both screens before approving; the code is a verification value, not a password.',
  },
  {
    id: 'encryption',
    question: 'Is the audio encrypted?',
    answer:
      'The control channel is encrypted and authenticated. UDP audio sent over Wi-Fi or USB tethering is not encrypted or authenticated; the protocol provides no confidentiality or integrity for those packets.',
  },
]

const { isOpen, toggle } = useAccordion()
</script>

<template>
  <section
    id="faq"
    class="flex flex-col items-center justify-center gap-y-10 px-4 py-20 sm:px-6 sm:py-24 lg:px-10"
  >
    <SectionHeading eyebrow="FAQ" title="Common questions" />

    <ul class="flex w-full max-w-3xl flex-col gap-y-3">
      <FaqItem
        v-for="entry in questions"
        :key="entry.id"
        :question="entry.question"
        :answer="entry.answer"
        :is-open="isOpen(entry.id)"
        @toggle="toggle(entry.id)"
      />
    </ul>
  </section>
</template>
