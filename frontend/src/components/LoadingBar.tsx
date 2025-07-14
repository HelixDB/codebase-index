'use client';

import { motion } from 'framer-motion';

export default function LoadingBar() {
  return (
    <div className="w-full h-1 bg-surface rounded-full overflow-hidden">
      <motion.div
        className="h-full bg-accent"
        initial={{ x: '-100%' }}
        animate={{ x: '100%' }}
        transition={{ repeat: Infinity, duration: 1, ease: 'linear' }}
      />
    </div>
  );
}