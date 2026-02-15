import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import QRCode from 'react-qr-code';
import './App.css';

function App() {
  const [screen, setScreen] = useState('qr'); // 'qr' or 'message'
  const [qrCode, setQrCode] = useState('');
  const [contact, setContact] = useState('');
  const [message, setMessage] = useState('');
  const [mediaFile, setMediaFile] = useState(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [success, setSuccess] = useState('');
  const [isReady, setIsReady] = useState(false);

  useEffect(() => {
    // Initialize WhatsApp connection
    initializeWhatsApp();

    // Listen for QR code events
    const setupListeners = async () => {
      const qrUnlisten = await listen('qr-code', (event) => {
        console.log('QR Code received:', event.payload);
        setQrCode(event.payload.code);
        setError('');
      });

      const authUnlisten = await listen('auth-success', async () => {
        console.log('Authentication successful!');
        
        // Wait a bit for full connection
        setTimeout(async () => {
          const ready = await invoke('is_bot_ready');
          setIsReady(ready);
          if (ready) {
            setScreen('message');
            setSuccess('WhatsApp connected and ready!');
            setTimeout(() => setSuccess(''), 3000);
          }
        }, 2000);
      });

      // Return cleanup function
      return () => {
        qrUnlisten();
        authUnlisten();
      };
    };

    const cleanup = setupListeners();

    return () => {
      cleanup.then((fn) => fn && fn());
    };
  }, []);

  const initializeWhatsApp = async () => {
    try {
      setLoading(true);
      await invoke('init_whatsapp');
      console.log('WhatsApp initialization started');
    } catch (err) {
      console.error('Failed to initialize WhatsApp:', err);
      setError(`Failed to initialize: ${err}`);
    } finally {
      setLoading(false);
    }
  };

  const handleSendMessage = async () => {
    if (!contact.trim()) {
      setError('Please enter a contact number');
      return;
    }

    if (!message.trim() && !mediaFile) {
      setError('Please enter a message or select a media file');
      return;
    }

    try {
      setLoading(true);
      setError('');
      
      // Check if bot is ready before sending
      const ready = await invoke('is_bot_ready');
      if (!ready) {
        setError('WhatsApp is not fully connected yet. Please wait...');
        setLoading(false);
        return;
      }
      
      let messageId;

      if (mediaFile) {
        // Send media message
        const mediaType = getMediaType(mediaFile);
        messageId = await invoke('send_media_message', {
          contact: contact,
          messageText: message,
          mediaPath: mediaFile,
          mediaType: mediaType,
        });
      } else {
        // Send text message
        messageId = await invoke('send_message', {
          contact: contact,
          message: message,
        });
      }

      console.log('Message sent with ID:', messageId);
      setSuccess('Message sent successfully!');
      
      // Clear form
      setMessage('');
      setMediaFile(null);
      
      // Clear success message after 3 seconds
      setTimeout(() => setSuccess(''), 3000);
    } catch (err) {
      console.error('Failed to send message:', err);
      setError(`Failed to send message: ${err}`);
    } finally {
      setLoading(false);
    }
  };

  const handleSelectMedia = async () => {
    try {
      const selected = await open({
        multiple: false,
        filters: [
          {
            name: 'Media Files',
            extensions: ['jpg', 'jpeg', 'png', 'gif', 'webp', 'mp4', 'mov', 'avi', 'pdf', 'docx', 'xlsx', 'txt', 'zip']
          }
        ]
      });

      if (selected && typeof selected === 'string') {
        setMediaFile(selected);
        setError('');
      }
    } catch (err) {
      console.error('Failed to select media:', err);
      setError('Failed to select media file');
    }
  };

  const handleRemoveMedia = () => {
    setMediaFile(null);
  };

  const getMediaType = (filePath) => {
    const extension = filePath.split('.').pop()?.toLowerCase() || '';
    
    if (['jpg', 'jpeg', 'png', 'gif', 'webp'].includes(extension)) {
      return 'image';
    } else if (['mp4', 'mov', 'avi', 'mkv'].includes(extension)) {
      return 'video';
    } else if (['mp3', 'ogg', 'wav', 'm4a'].includes(extension)) {
      return 'audio';
    } else {
      return 'document';
    }
  };

  const getFileName = (filePath) => {
    return filePath.split(/[\\/]/).pop() || filePath;
  };

  const getMediaIcon = (filePath) => {
    const type = getMediaType(filePath);
    const icons = {
      image: '🖼️',
      video: '🎥',
      audio: '🎵',
      document: '📄'
    };
    return icons[type] || '📄';
  };

  return (
    <div className="app">
      {screen === 'qr' ? (
        /* QR Code Screen */
        <div className="screen qr-screen">
          <div className="container">
            <h1>🔗 Connect WhatsApp</h1>
            <p className="subtitle">Scan the QR code with your WhatsApp mobile app</p>

            {loading && (
              <div className="loading">
                <div className="spinner"></div>
                <p>Initializing connection...</p>
              </div>
            )}

            {qrCode ? (
              <div className="qr-container">
                <div className="qr-code-wrapper">
                  <QRCode
                    value={qrCode}
                    size={256}
                    style={{ height: "auto", maxWidth: "100%", width: "100%" }}
                    viewBox={`0 0 256 256`}
                    fgColor="#000000"
                    bgColor="#ffffff"
                    level="L"
                  />
                </div>
                <div className="qr-instructions">
                  <h3>📱 How to scan:</h3>
                  <ol>
                    <li>Open <strong>WhatsApp</strong> on your phone</li>
                    <li>Tap <strong>Menu</strong> (⋮) or <strong>Settings</strong> (⚙️)</li>
                    <li>Select <strong>Linked Devices</strong></li>
                    <li>Tap <strong>Link a Device</strong></li>
                    <li>Point your phone at this screen to capture the code</li>
                  </ol>
                </div>
              </div>
            ) : (
              !loading && (
                <div className="waiting">
                  <div className="pulse"></div>
                  <p>Waiting for QR code...</p>
                  <p className="waiting-hint">Make sure you're connected to the internet</p>
                </div>
              )
            )}

            {error && (
              <div className="alert alert-error">
                ❌ {error}
              </div>
            )}
          </div>
        </div>
      ) : (
        /* Message Screen */
        <div className="screen message-screen">
          <div className="container">
            <div className="header">
              <h1>💬 Send WhatsApp Message</h1>
              <div className={`status-badge ${isReady ? 'ready' : 'connecting'}`}>
                {isReady ? '✓ Ready' : '⏳ Connecting...'}
              </div>
            </div>

            {success && (
              <div className="alert alert-success">
                ✓ {success}
              </div>
            )}

            {error && (
              <div className="alert alert-error">
                ❌ {error}
              </div>
            )}

            {!isReady && (
              <div className="alert alert-warning">
                ⏳ Please wait, establishing connection...
              </div>
            )}

            <div className="form">
              <div className="form-group">
                <label htmlFor="contact">
                  📞 Contact Number
                  <span className="hint">(e.g., 1234567890 or +1234567890)</span>
                </label>
                <input
                  id="contact"
                  type="text"
                  placeholder="Enter phone number"
                  value={contact}
                  onChange={(e) => setContact(e.target.value)}
                  disabled={loading || !isReady}
                />
              </div>

              <div className="form-group">
                <label htmlFor="message">
                  ✉️ Message
                  {mediaFile && <span className="hint">(optional with media)</span>}
                </label>
                <textarea
                  id="message"
                  placeholder="Type your message here..."
                  value={message}
                  onChange={(e) => setMessage(e.target.value)}
                  disabled={loading || !isReady}
                  rows={5}
                />
              </div>

              <div className="form-group">
                <label>📎 Attach Media (Optional)</label>
                
                {mediaFile ? (
                  <div className="media-preview">
                    <div className="media-info">
                      <span className="media-icon">
                        {getMediaIcon(mediaFile)}
                      </span>
                      <span className="media-name">{getFileName(mediaFile)}</span>
                    </div>
                    <button
                      className="btn btn-remove"
                      onClick={handleRemoveMedia}
                      disabled={loading || !isReady}
                    >
                      ✕ Remove
                    </button>
                  </div>
                ) : (
                  <button
                    className="btn btn-secondary"
                    onClick={handleSelectMedia}
                    disabled={loading || !isReady}
                  >
                    📁 Select File
                  </button>
                )}
              </div>

              <button
                className="btn btn-primary"
                onClick={handleSendMessage}
                disabled={loading || !isReady || (!message.trim() && !mediaFile) || !contact.trim()}
              >
                {loading ? (
                  <>
                    <div className="spinner-small"></div>
                    <span>Sending...</span>
                  </>
                ) : (
                  <>
                    <span>🚀 Send Message</span>
                  </>
                )}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

export default App;