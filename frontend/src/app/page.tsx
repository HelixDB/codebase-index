'use client';

import { useState } from 'react';
import Chat from '../components/Chat';
import { ModelChatHistory } from '@/lib/types';

export default function HomePage() {
  const [chatHistory, setChatHistory] = useState<ModelChatHistory>({
    messages: []
  });

  return (
    <div className="relative h-screen w-full bg-background overflow-hidden">
      <main className="w-full h-full flex flex-col">
            <div className="flex flex-1 overflow-hidden">
              <div className="flex-1 overflow-auto">
                <Chat 
                  chatHistory={chatHistory}
                  setChatHistory={setChatHistory} 
                />
              </div>
            </div>
      </main>
    </div>
  );
}
