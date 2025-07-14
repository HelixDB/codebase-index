'use client';

import React, { useState, useRef, useEffect } from 'react';
import { Message, ModelChatHistory } from '@/lib/types';
import MessageBubble from './MessageBubble';
import TypingIndicator from './TypingIndicator';
import { motion, useAnimation } from 'framer-motion';

interface ChatProps {
  chatHistory: ModelChatHistory;
  setChatHistory: React.Dispatch<React.SetStateAction<ModelChatHistory>>;
}

export default function Chat({ chatHistory, setChatHistory }: ChatProps) {
  const [messages, setMessages] = useState<Message[]>([]);
  const [input, setInput] = useState('');
  const [isTyping, setIsTyping] = useState(false);
  const chatContainerRef = useRef<HTMLDivElement>(null);
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const [userHasScrolled, setUserHasScrolled] = useState(false);
  const textareaAnimation = useAnimation();
  const shineAnimation = useAnimation();
  const [init, setInit] = useState(true);

  const handleScroll = () => {
    const container = chatContainerRef.current;
    if (container) {
      const isAtBottom = container.scrollHeight - container.scrollTop <= container.clientHeight + 1;
      setUserHasScrolled(!isAtBottom);
    }
  };

  useEffect(() => {
    const container = chatContainerRef.current;
    if (container) {
      if (!userHasScrolled) {
        container.scrollTo({
          top: container.scrollHeight,
          behavior: 'smooth'
        });
      }
    }
  }, [messages, isTyping, userHasScrolled]);

  useEffect(() => {
    if (init) {
      const existingMessages = chatHistory.messages || [];
      setMessages(existingMessages);
      setIsTyping(false);
      setUserHasScrolled(false);
      setInit(false);
    }
  }, [chatHistory, init]);
  
  useEffect(() => {
    if (messages.length > 0) {
        setChatHistory(prev => ({
          ...prev,
          messages: messages
        }));
    }
  }, [messages, setChatHistory]);

  useEffect(() => {
    if (textareaRef.current) {
      textareaRef.current.focus();
    }
    
    const handleGlobalKeyDown = (e: KeyboardEvent) => {
      const selection = window.getSelection();
      if ((e.metaKey || e.ctrlKey) && selection && selection.toString().length > 0) {
        e.stopPropagation();
        
        if (document.activeElement === textareaRef.current && 
            ['c', 'x', 'a'].includes(e.key.toLowerCase())) {
          return;
        }
      }
    };
    
    window.addEventListener('keydown', handleGlobalKeyDown, true);
    
    return () => {
      window.removeEventListener('keydown', handleGlobalKeyDown, true);
    };
  }, []);

  useEffect(() => {
    const handleGlobalKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement;

      if (
        target.tagName === 'INPUT' ||
        target.tagName === 'TEXTAREA' ||
        target.isContentEditable
      ) {
        return;
      }
      
      if (event.key.length === 1 || event.key === 'Backspace') {
        if (textareaRef.current) {
          textareaRef.current.focus();
        }
      }
    };

    window.addEventListener('keydown', handleGlobalKeyDown);

    return () => {
      window.removeEventListener('keydown', handleGlobalKeyDown);
    };
  }, []);

  const handleSend = async () => {
    if (!input.trim()) return;

    setUserHasScrolled(false);

    const userMessage: Message = { role: 'user', content: input, id: `user-${Date.now()}` };
    
    // Update messages with the user message
    setMessages(currentMessages => [...currentMessages, userMessage]);
    
    setChatHistory(prevHistory => ({
      ...prevHistory,
      messages: [...(prevHistory.messages || []), userMessage]
    }));

    
    setInput('');
    setIsTyping(true);

    const response = await fetch('/api/chat', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ messages: [...messages, userMessage] }),
    });

    if (!response.body) {
      setIsTyping(false);
      return;
    }

    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    const assistantMessageId = `assistant-${Date.now()}`;
    
    // Add an empty assistant message that will be updated with content
    setMessages(currentMessages => [...currentMessages, { 
      role: 'assistant', 
      content: '', 
      id: assistantMessageId, 
      streaming: true 
    }]);

    try {
      while (true) {
        const { done, value } = await reader.read();
        
        if (done) {
          // Mark message as no longer streaming when done
          setMessages(currentMessages => {
            const lastIndex = currentMessages.length - 1;
            if (lastIndex >= 0) {
              const updatedMessages = [...currentMessages];
              updatedMessages[lastIndex] = {
                ...updatedMessages[lastIndex],
                streaming: false
              };
              return updatedMessages;
            }
            return currentMessages;
          });
          
          setIsTyping(false);
          break;
        }
        
        // Decode the chunk and add it to the message content
        const text = decoder.decode(value, { stream: true });
        
        // Update the assistant message with new content
        setMessages(currentMessages => {
          const lastIndex = currentMessages.length - 1;
          if (lastIndex >= 0) {
            const updatedMessages = [...currentMessages];
            updatedMessages[lastIndex] = {
              ...updatedMessages[lastIndex],
              content: updatedMessages[lastIndex].content + text
            };
            return updatedMessages;
          }
          return currentMessages;
        });
      }
      
      // Update chat history with the final message
      setMessages(currentMessages => {
        const lastMessage = currentMessages[currentMessages.length - 1];
        if (lastMessage) {
          setChatHistory(prev => ({
            ...prev,
            messages: [...(prev.messages || []).filter(m => m.id !== lastMessage.id), {
              ...lastMessage,
              streaming: false
            }]
          }));
        }
        return currentMessages;
      });
      
    } catch (error) {
      console.error('Error reading stream:', error);
      setIsTyping(false);
    }
  };

  return (
    <div className="flex flex-col h-full overflow-hidden bg-background text-primary">
      {messages.length === 0 ? (
        <div className="flex flex-col items-center justify-center h-full p-4 text-center">
          <motion.div 
            initial={{ opacity: 0, y: 20 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.5 }}
            className="max-w-lg"
            style={{ position: 'relative', borderRadius: '2rem', maxWidth: '100%', width: 'auto', height: 'fit-content', bottom: '60px'}}
          >
            <h1 className="text-3xl font-bold mb-2">What&apos;s on your mind?</h1>
          </motion.div>
        </div>
      ) : (
        <div ref={chatContainerRef} onScroll={handleScroll} className="flex-1 p-4 space-y-4 overflow-y-auto" style={{ paddingBottom: '140px', paddingTop: '10px' }}>
          {messages.map((msg, i) => (
            <MessageBubble key={i} message={msg} isUser={msg.role === 'user'} />
          ))}
          {isTyping && <TypingIndicator />}
          <div ref={messagesEndRef} />
        </div>
      )}
      
      <div className="justify-center items-center" style={{ height: 'fit-content', width: '600px', position: 'absolute', bottom: '-20px', left: '50%', transform: 'translateX(-50%)' }}>
        <div className="flex flex-col items-center w-full" style={{ maxWidth: '600px', height: 'fit-content' }}>
          <motion.div
            animate={textareaAnimation}
            className="flex justify-center items-center p-2 w-full"
            style={{ marginBottom: '50px', marginTop: '0' }}
          >
            <div className="messageBubbleGlass" style={{ position: 'relative', overflow: 'hidden', width: '100%', maxHeight: '144px', height: 'auto', minHeight: '60px'}}>
              <motion.div
                animate={shineAnimation}
                onAnimationComplete={() => shineAnimation.set({ x: '-100%' })}
                style={{
                  position: 'absolute',
                  top: 0,
                  left: 0,
                  width: '100%',
                  height: '100%',
                  pointerEvents: 'none',
                  background: 'linear-gradient(to right, transparent 20%, rgba(255, 255, 255, 0.05) 30%, rgba(255, 255, 255, 0.2) 50%, rgba(255, 255, 255, 0.05) 70%, transparent 80%)',
                  x: '-100%',
                  zIndex: 0,
                }}
              />
              <textarea
                onFocus={() => {
                  textareaAnimation.start({
                    scale: [0.99, 0.98, 1],
                    transition: { duration: 0.3, ease: 'easeInOut' }
                  });
                  shineAnimation.start({
                    x: '100%',
                    transition: { duration: 0.3, ease: 'easeInOut' }
                  });
                }}
                ref={(el) => {
                  textareaRef.current = el;
                  if (el) {
                    // Initial height adjustment
                    setTimeout(() => {
                      const lineHeight = 24; // Approximate line height
                      const maxHeight = lineHeight * 6; // 6 lines max
                      
                      // Reset height to content
                      el.style.height = 'auto';
                      
                      // Set new height, capped at maxHeight
                      if (el.scrollHeight <= maxHeight) {
                        el.style.height = el.scrollHeight + 'px';
                        el.style.overflowY = 'hidden';
                      } else {
                        el.style.height = maxHeight + 'px';
                        el.style.overflowY = 'auto';
                      }
                    }, 0);
                  }
                }}
                value={input}
                onKeyDown={(e: React.KeyboardEvent<HTMLTextAreaElement>) => {
                  // Only handle Enter key for sending message
                  if (e.key === 'Enter' && !e.shiftKey) {
                    e.preventDefault();
                    handleSend();
                  }
                }}
                onChange={(e: React.ChangeEvent<HTMLTextAreaElement>) => {
                  setInput(e.target.value);
                  
                  const lineHeight = 24; // Approximate line height
                  const maxHeight = lineHeight * 6; // 6 lines max
                  
                  // Reset height to content
                  e.target.style.height = 'auto';
                  
                  // Set new height, capped at maxHeight
                  if (e.target.scrollHeight <= maxHeight) {
                    e.target.style.height = e.target.scrollHeight + 'px';
                    e.target.style.overflowY = 'hidden';
                  } else {
                    e.target.style.height = maxHeight + 'px';
                    e.target.style.overflowY = 'auto';
                  }
                }}

                className="bg-transparent text-white w-full resize-none focus:outline-none"
                placeholder="Ask me anything"
                style={{ 
                  color: 'rgba(255, 255, 255, 0.90)',
                  padding: '1em 1em',
                  minHeight: '24px',     /* Start with single line height */
                  maxHeight: '144px',    /* Max height (6 lines at 24px) */
                  boxSizing: 'border-box',
                  display: 'block',
                  width: '95%',
                  height: '90%',
                  margin: '0 auto',
                  border: 'none',
                  lineHeight: '24px',    /* Consistent line height */
                  transition: 'height 0.1s ease',
                  position: 'relative',
                  zIndex: 1,
                }}
                rows={1}
              />
            </div>
        </motion.div>
      </div>
      <svg style={{ display: 'none' }}>
        <filter id="message-glass" x="0%" y="0%" width="100%" height="100%">
          <feTurbulence type="fractalNoise" baseFrequency="0.008 0.008" numOctaves="2" result="noise" />
          <feGaussianBlur in="noise" stdDeviation="0.01" result="blur" />
          <feDisplacementMap in="SourceGraphic" in2="blur" scale="10" xChannelSelector="R" yChannelSelector="G" />
        </filter>
      </svg>
    </div>
  </div>
  );
}