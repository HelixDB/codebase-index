'use client';

import { motion } from 'framer-motion';

export default function TypingIndicator() {
  return (
    <div className="flex items-center p-2 my-2">
      <motion.div 
        className="w-full h-0.5 bg-accent rounded-full" 
        animate={{
          scaleX: [0, 1, 0],
        }}
        transition={{
          duration: 1.5,
          ease: "easeInOut",
          repeat: Infinity,
        }}
      />
    </div>
  );
}