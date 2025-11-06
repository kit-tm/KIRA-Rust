// ==UserScript==
// @name         NodeId Replacer
// @namespace    http://tampermonkey.net/
// @version      1.0
// @description  Replace Node-Ids with there corresponding Topology-IDs.
// @author       Julius Rüberg with Gemini 2.5 Pro
// @match        *://*/*
// @grant        none
// @run-at       document-start
// ==/UserScript==

(function() {
    "use strict";

    // --- PASTE YOUR KEY DATA HERE ---
    // This is the output of `ntest > NODES`
    const defaultKeyData = `
 k0 E3E7C2094CAC629F6FBED82C07CD
 k1 F72842485E3A0A5D2F346BAA9455
 k2 EB1167A9C3787C65C1E582E2E662
 k3 F7C14DA5E709D4713D60C8A70639
 k4 E4439558867F5BA91FAF7A024204
 k5 23A78133287637EBDCD9E87A1613
 k6 1846C17C627923C6612F48268673
 k7 FCBD40212EF7CCA5A5A19E4D6E3C
 k8 B486FB97D43588561712E8E5216A
 k9 259FE6F4590B9A164106CF6A659E
k10 12E0BAD640FB19488DEC4F65D4D9
k11 5487AF19922AD9B8A714E61A441C
k12 5A9219C78DF48F4FF31E78DE5857
k13 A3F29C6316B950F244556F25E2A2
k14 8D72F77383C13458A748E9BB17BC
k15 8577DD84F39E71545A137A1D5006
k16 EB20CE164DBA0FF18E0242AF9FC3
k17 17E003983CA8EA7E9D498C778EA6
k18 B5D366194CB1D71037D1B83E90EC
k19 A011AB0C1681C8F8E3D0D3290A4C
    `;

    // --- SCRIPT STATE ---
    let substitutionMap = new Map();
    let hexRegex = new RegExp('a^', ''); // A regex that will never match anything initially
    let originalValues = new WeakMap();

    // --- UI CREATION ---

    // --- HELPER FUNCTION TO ADD CSS ---
    function addGlobalStyle(css) {
        const head = document.head || document.getElementsByTagName('head')[0];
        if (!head) { return; }
        const style = document.createElement('style');
        style.type = 'text/css';
        style.innerHTML = css;
        head.appendChild(style);
    }

    function createUI() {
        // Add styles for the UI elements
        addGlobalStyle(`
            #hex-replacer-btn {
                position: fixed;
                bottom: 20px;
                right: 20px;
                z-index: 9999;
                background-color: #007bff;
                color: white;
                border: none;
                border-radius: 50%;
                width: 50px;
                height: 50px;
                font-size: 24px;
                cursor: pointer;
                box-shadow: 0 4px 8px rgba(0,0,0,0.2);
            }
            #hex-replacer-panel {
                position: fixed;
                top: 50%;
                left: 50%;
                transform: translate(-50%, -50%);
                z-index: 10000;
                background: #f9f9f9;
                border: 1px solid #ccc;
                border-radius: 8px;
                padding: 20px;
                box-shadow: 0 5px 15px rgba(0,0,0,0.3);
                display: none; /* Hidden by default */
                width: 400px;
            }
            #hex-replacer-panel textarea {
                width: 100%;
                height: 200px;
                margin-bottom: 10px;
                border: 1px solid #ddd;
                border-radius: 4px;
            }
            #hex-replacer-panel button {
                padding: 10px 15px;
                border: none;
                border-radius: 4px;
                cursor: pointer;
                margin-right: 10px;
            }
            #hex-replacer-apply {
                background-color: #28a745;
                color: white;
            }
            #hex-replacer-close {
                background-color: #dc3545;
                color: white;
            }
        `);

        // Create the settings button
        const settingsButton = document.createElement('button');
        settingsButton.id = 'hex-replacer-btn';
        settingsButton.innerHTML = '&#9881;'; // Gear icon
        document.body.appendChild(settingsButton);

        // Create the settings panel
        const panel = document.createElement('div');
        panel.id = 'hex-replacer-panel';
        panel.innerHTML = `
            <h3>Hex Replacer Settings</h3>
            <p>Paste your key-value data below:</p>
            <textarea id="hex-replacer-data">${defaultKeyData.trim()}</textarea>
            <button id="hex-replacer-apply">Apply</button>
            <button id="hex-replacer-close">Close</button>
        `;
        document.body.appendChild(panel);

        // Add event listeners
        settingsButton.addEventListener('click', () => {
            panel.style.display = 'block';
        });

        document.getElementById('hex-replacer-close').addEventListener('click', () => {
            panel.style.display = 'none';
        });

        document.getElementById('hex-replacer-apply').addEventListener('click', () => {
            const newData = document.getElementById('hex-replacer-data').value;
            revertAllChanges(); // Revert old changes before applying new ones
            initializeReplacer(newData);
            panel.style.display = 'none';
        });
    }



    // --- SCRIPT LOGIC ---

    function initializeReplacer(keyData) {
        console.info("Initializing replacer with new data...");
        substitutionMap.clear();
        keyData
            .trim()
            .split('\n')
            .forEach(line => {
                const parts = line.trim().split(/\s+/);
                if (parts.length === 2) {
                    const key = parts[0];
                    const value = parts[1].toUpperCase();
                    substitutionMap.set(value, key);
                }
            });

        const allHexKeys = Array.from(substitutionMap.keys());
        if (allHexKeys.length > 0) {
            // Create a single regex to find any of the hex strings.
            // This is much more efficient than iterating over the map for every text node.
            hexRegex = new RegExp(allHexKeys.join('|'), 'g');
            console.debug("NodeId regex created:", hexRegex);
        } else {
            hexRegex = new RegExp('a^', '');
        }
        scanAndReplace(document.body);
    }

    function revertAllChanges() {
        console.info("Reverting all previous changes...");
        const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
        let node;
        while (node = walker.nextNode()) {
            if (originalValues.has(node)) {
                node.nodeValue = originalValues.get(node);
            }
        }
        // Clear the map for the next run
        originalValues = new WeakMap();
    }


    function getReplacedText(text) {
        if (!hexRegex.test(text)) {
            return null;
        }
        return text.replace(hexRegex, (matchedHex) => {
            const replacementKey = substitutionMap.get(matchedHex.toUpperCase());
            //console.debug(`Found match: "${matchedHex}", replacing with: "${replacementKey}"`);
            return replacementKey || matchedHex;
        });
    }

    function scanAndReplace(targetNode) {
        console.debug("Scanning node for replacement:", targetNode);
        const walker = document.createTreeWalker(targetNode, NodeFilter.SHOW_TEXT);

        let node;
        const nodesToProcess = [];
        while (node = walker.nextNode()) {
            const parent = node.parentElement;
            const parentTag = parent.tagName;
            const parentId = parent.id;
            if (parentTag === "SCRIPT" || parentTag === "STYLE" || parentId.startsWith('hex-replacer-')) {
                continue;
            }
            nodesToProcess.push(node);
        }
        // process the collected nodes.
        for (const textNode of nodesToProcess) {
            const newText = getReplacedText(textNode.nodeValue);
            if (newText !== null && textNode.nodeValue !== newText) {
                textNode.nodeValue = newText;
            }
        }
    }

    // watch for changes to the page.
    const observer = new MutationObserver((mutationsList) => {
        console.debug("MutationObserver detected", mutationsList.length, "mutations.");

        for (const mutation of mutationsList) {
            // new elements were added to the page
            if (mutation.type === "childList" && mutation.addedNodes.length) {
                // ...scan each new element for text to replace.
                mutation.addedNodes.forEach(newNode => {
                    if (newNode.nodeType === Node.ELEMENT_NODE) {
                        scanAndReplace(newNode);
                    } else if (newNode.nodeType === Node.TEXT_NODE) {
                        const newText = getReplacedText(newNode.nodeValue);
                        if (newText !== null) {
                            newNode.nodeValue = newText;
                        }
                    }
                });
            }
            // content of an existing element changed
            else if (mutation.type === "characterData") {
                scanAndReplace(mutation.target.parentElement);
            }
        }
    });

    function observe() {
        if (!document.body) {
            window.addEventListener("DOMContentLoaded", () => observe(), { once: true });
            return;
        }

        console.info("NodeId Replacer: Starting to observe body for changes.");
        observer.observe(document.body, {
            childList: true,
            subtree: true,
            characterData: true
        });
    }


    console.info("NodeId replacer: Initializing...");
    const readyState = document.readyState;
    if (readyState === "interactive" || readyState === "complete") {
        createUI();
        initializeReplacer(defaultKeyData);
        observe();
    } else {
        window.addEventListener("DOMContentLoaded", () => {
            createUI();
            initializeReplacer(defaultKeyData);
            observe();
        }, { once: true });
    }

})();
