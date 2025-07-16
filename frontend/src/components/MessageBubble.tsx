'use client';

import React, { useEffect, useState, useRef } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { motion, AnimatePresence } from 'framer-motion';
import { Message } from '@/lib/types';
import { ChevronDown, ChevronUp, Copy, Check } from 'lucide-react';
import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import { vscDarkPlus } from 'react-syntax-highlighter/dist/cjs/styles/prism';

interface MessageBubbleProps {
  message: Message;
  isUser: boolean;
}

// Animated ellipsis component
function AnimatedEllipsis() {
  const [dots, setDots] = useState('');
  
  useEffect(() => {
    const interval = setInterval(() => {
      setDots(prev => {
        if (prev === '') return '.';
        if (prev === '.') return '..';
        if (prev === '..') return '...';
        return '';
      });
    }, 200);
    
    return () => clearInterval(interval);
  }, []);
  
  return <span>{dots}</span>;
}

export default function MessageBubble({ message, isUser }: MessageBubbleProps) {
  // Ref to track the content container's dimensions
  const contentRef = useRef<HTMLDivElement>(null);
  const [thinkContent, setThinkContent] = useState<string | null>(null);
  const [displayContent, setDisplayContent] = useState<string>(message.content || '');
  const [isThinkingExpanded, setIsThinkingExpanded] = useState<boolean>(false);
  const [copiedCode, setCopiedCode] = useState<string | null>(null);

  // Reset the copied state after 2 seconds
  useEffect(() => {
    if (copiedCode) {
      const timer = setTimeout(() => {
        setCopiedCode(null);
      }, 500);
      return () => clearTimeout(timer);
    }
  }, [copiedCode]);
  
  // Function to handle copying code
  const handleCopyCode = (code: string) => {
    navigator.clipboard.writeText(code);
    setCopiedCode(code);
    setTimeout(() => {
      setCopiedCode(null);
    }, 500);
  };

  const handleExpandThinking = () => {
    const timer = setTimeout(() => {
    }, 300);
    setIsThinkingExpanded(!isThinkingExpanded);
    return () => clearTimeout(timer);
  };

  // Effect to handle content processing
  useEffect(() => {
    // Handle the display of content and think content
    if (message.content) {
      // First, try to extract think content using regex for complete <think> tags
      const thinkRegex = /<think>([\s\S]*?)<\/think>/s;
      const match = message.content.match(thinkRegex);
      
      if (match && match[1]) {
        // Complete <think></think> tags found
        setThinkContent(match[1].trim());
        
        // Remove the <think> tags and content from the display content
        const cleanedContent = message.content.replace(thinkRegex, '').trim();
        setDisplayContent(cleanedContent);
      } else {
        // Check for unclosed <think> tags
        const openTagIndex = message.content.indexOf('<think>');
        const closeTagIndex = message.content.indexOf('</think>');
        
        if (openTagIndex !== -1) {
          // We have an opening tag
          if (closeTagIndex === -1 || closeTagIndex < openTagIndex) {
            // Only opening tag without proper closing tag
            const beforeTag = message.content.substring(0, openTagIndex).trim();
            const thinkContent = message.content.substring(openTagIndex + 7).trim();
            
            setThinkContent(thinkContent);
            setDisplayContent(beforeTag);
          } else {
            // Both opening and closing tags in correct order
            const beforeTag = message.content.substring(0, openTagIndex).trim();
            const thinkContent = message.content.substring(openTagIndex + 7, closeTagIndex).trim();
            const afterTag = message.content.substring(closeTagIndex + 8).trim();
            
            setThinkContent(thinkContent);
            setDisplayContent(beforeTag + ' ' + afterTag);
          }
        } else if (closeTagIndex !== -1) {
          // Only closing tag without opening tag
          const displayContent = message.content.replace('</think>', '').trim();
          setThinkContent(null);
          setDisplayContent(displayContent);
        } else {
          // No think tags at all
          setThinkContent(null);
          setDisplayContent(message.content);
        }
      }
    } else {
      // For user messages, just display the content
      setThinkContent(null);
      setDisplayContent(message.content || '');
    }
  }, [message.content, isUser]);

  // Track the maximum height the container has reached during streaming
  const [maxHeight, setMaxHeight] = useState<number | null>(null);
  
  // Effect to track content height during streaming
  useEffect(() => {
    if (message.streaming && contentRef.current) {
      const currentHeight = contentRef.current.scrollHeight;
      
      // Only update if the current height is greater than the previous max
      setMaxHeight(prev => prev === null || currentHeight > prev ? currentHeight : prev);
    } else if (!message.streaming) {
      // Reset when streaming is done
      setMaxHeight(null);
    }
  }, [message.streaming, displayContent]);

  return (
    <div className="flex flex-col">
      {/* Think content displayed as a dropdown above the message bubble */}
      {thinkContent && !isUser && (
          <div
            className="flex justify-start mb-1"
            style={{ padding: '0.5rem 1rem'}}
          >
            <div 
              className="bg-gray-800 rounded-md overflow-hidden"
              style={{ maxWidth: '48%', width: 'fit-content' }}
            >
              {/* Dropdown header - styled like code block header */}
                <motion.div 
                  className="messageBubbleGlass think-button flex items-center justify-between px-3 py-2 cursor-pointer"
                  onClick={() => handleExpandThinking()}
                  whileTap={{ scale: 0.80 }}
                  animate={{
                    scale: isThinkingExpanded ? [0.95, 1] : 1
                  }}
                  transition={{
                    duration: 0.2,
                    ease: "easeInOut"
                  }}
                >
                  <div className="flex items-center text">
                    {displayContent === "" ? (
                      <span style={{paddingRight: '8px'}}>
                        Thinking <AnimatedEllipsis />
                      </span>
                    ) : (
                      <span style={{paddingRight: '8px'}}>Thoughts</span>
                    )}
                  </div>
                  {isThinkingExpanded ? 
                    <ChevronUp className="text" size={16} /> : 
                    <ChevronDown className="text" size={16} />
                  }
                </motion.div>
                {/* Dropdown content */}
                <AnimatePresence>
                {isThinkingExpanded && (
                  <motion.div
                    key="thinking-content"
                    initial={{ height: 0, opacity: 0 }}
                    animate={{ height: 'auto', opacity: 1 }}
                    exit={{ height: 0, opacity: 0 }}
                    transition={{ duration: 0.2 }}
                    style={{ overflow: 'hidden', paddingTop: '5px' }}
                  >
                    <div
                      className="font-mono text-gray-300 overflow-auto rounded-b-md"
                      style={{
                        padding: '0.5rem 1rem',
                        backgroundColor: 'rgba(50, 50, 50, 0.3)',
                        borderRadius: '2rem',
                      }}
                    >
                      <pre className="whitespace-pre-wrap">
                        <code>{thinkContent}</code>
                      </pre>
                    </div>
                  </motion.div>
                )}
                </AnimatePresence>
            </div>
          </div>
      )}

      {/* Regular message bubble */}
      {displayContent !== '' && (<div
        className={`flex my-1 ${isUser ? 'justify-end' : 'justify-start'}`}
        style={{ padding: '0.5rem 1rem'}}
      >
        <div
          className={`messageBubbleGlass`}
          style={{ position: 'relative', borderRadius: '2rem', minWidth: '20%', maxWidth: '48%', width: 'auto', height: 'auto'}}
        >
          <div style={{ padding: '1rem 1rem', position: 'relative'}}>
            <div 
              ref={contentRef}
              className="prose prose-invert prose-sm w-full" 
              style={{ 
                maxWidth: '100%', 
                overflowWrap: 'break-word',
                minHeight: message.streaming && maxHeight ? `${maxHeight}px` : 'auto',
                position: 'relative',
                zIndex: 0,
                transition: 'none',
                overflow: 'visible'
              }}>
              <ReactMarkdown 
                remarkPlugins={[remarkGfm]}
                components={{
                  // eslint-disable-next-line @typescript-eslint/no-explicit-any
                  code({className, children, node, ...props}: any) {
                    const match = /language-(\w+)/.exec(className || '');
                    const codeContent = String(children || '');
                    const position = node.position;
                    const start = position?.start.offset;
                    const end = position?.end.offset;
                    const rawContent = displayContent.substring(start, end)
                    
                    const isInline = rawContent.includes("`") && !rawContent.includes("```");
                    
                    if (!isInline) {
                      // This is a code block (not inline code)
                      return (
                        <div className="relative overflow-hidden" style={{ position: 'relative', backgroundColor: 'rgba(50, 50, 50, 0.4)', padding: '1rem', minWidth: '3em', borderRadius: '2rem', maxWidth: '100%', overflowWrap: 'break-word', wordBreak: 'break-word', overflowX: 'hidden', width: 'fit-content' }}>
                          <motion.div 
                            onClick={() => handleCopyCode(codeContent)}
                            className="messageBubbleGlass model-button flex items-center justify-between cursor-pointer rounded-full"
                            aria-label="Copy code"
                            style={{ color: 'white', padding: '0.15rem', width: '30px', height: '30px', display: 'flex', alignItems: 'center', justifyContent: 'center', position: 'absolute', top: '1em', right: '1em' }}
                            animate={{
                              scale: copiedCode === codeContent ? [0.7, 1] : 1
                            }}
                            transition={{
                              duration: 0.2,
                              ease: "easeInOut"
                            }}
                          >
                            {copiedCode === codeContent ? 
                              <Check size={12}/> : 
                              <Copy size={12}/>
                            }
                          </motion.div>
                          <svg style={{display: 'none'}}>
                            <filter id="container-glass" x="0%" y="0%" width="100%" height="100%">
                              <feTurbulence type="fractalNoise" baseFrequency="0.008 0.008" numOctaves="2" seed="92" result="noise" />
                              <feGaussianBlur in="noise" stdDeviation="0.05" result="blur" />
                              <feDisplacementMap in="SourceGraphic" in2="blur" scale="5" xChannelSelector="R" yChannelSelector="G" />
                            </filter>
                          </svg> 
                          <div style={{ overflow: 'hidden', maxWidth: '100%', overflowX: 'hidden' }}>
                            <SyntaxHighlighter
                              language={match ? match[1] : 'bash'}
                              style={vscDarkPlus}
                              customStyle={{ 
                                background: 'transparent',
                                padding: '0.5rem',
                                paddingRight: '40px',
                                wordWrap: 'break-word',
                                whiteSpace: 'pre-wrap',
                                overflowWrap: 'break-word',
                                wordBreak: 'break-word',
                                width: 'auto',
                                maxWidth: '100%',
                                display: 'block',
                                overflow: 'hidden',
                                overflowX: 'hidden'
                              }}
                              wrapLongLines={true}
                              wrapLines={true}
                              codeTagProps={{
                                style: {
                                  wordBreak: 'break-word',
                                  whiteSpace: 'pre-wrap',
                                  overflowWrap: 'break-word'
                                }
                              }}
                            >
                              {codeContent}
                            </SyntaxHighlighter>
                          </div>
                        </div>
                      );
                    }
                    return <code className={className} {...props} style={{ backgroundColor: 'rgba(50, 50, 50, 0.4)', padding: '1px', borderRadius: '2rem', paddingLeft: '8px', paddingRight: '8px' }}>{children}</code>;
                  },
                  // Custom link component to handle long URLs
                  // eslint-disable-next-line @typescript-eslint/no-explicit-any
                  a({children, href, ...props}: any) {
                    return (
                      <a 
                        href={href} 
                        target="_blank" 
                        rel="noopener noreferrer"
                        style={{
                          wordBreak: 'break-word',
                          overflowWrap: 'break-word',
                          textDecoration: 'underline',
                          color: 'rgba(91, 108, 255, 0.90)',
                          position: 'relative',
                          zIndex: 1
                        }}
                        {...props}
                      >
                        {children}
                      </a>
                    );
                  }
                }}
              >
                {displayContent}
              </ReactMarkdown>
              <svg style={{ display: 'none' }}>
                <filter id="message-glass" x="0%" y="0%" width="100%" height="100%">
                  <feTurbulence type="fractalNoise" baseFrequency="0.008 0.008" numOctaves="2" seed="92" result="noise" />
                  <feGaussianBlur in="noise" stdDeviation="0.05" result="blur" />
                  <feDisplacementMap in="SourceGraphic" in2="blur" scale="50" xChannelSelector="R" yChannelSelector="G" />
                </filter>
              </svg>
            </div>
          </div>
        </div>
      </div>)}
    </div>
  );
}
