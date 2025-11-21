# **Scarlet Graphics Subsystem Architecture (v2)**

**Subject:** Graphics Buffer as a First-Class Kernel Object

**Status:** Approved

**Date:** 2025-01-20

## **1\. Overview**

This document outlines the revised architecture for the Scarlet Graphics Subsystem. The core design decision is to promote **Graphics Buffers** (e.g., dumb buffers, surfaces) to **First-Class Kernel Objects**.

By treating graphics buffers as KernelObject::GraphicsBuffer, we leverage the operating system's native HandleTable for resource lifecycle management, access control, and inter-process communication. Additionally, the Linux ABI layer (DrmFile) maintains its own strong references to these objects to ensure safety during cross-task sharing (e.g., via fork or IPC).

## **2\. Core Design Principles**

### **2.1 Unified Resource Management (Double Ownership)**

* **Principle:** Resources are managed primarily by the creating task's HandleTable, but shared sessions (like DRM) must maintain their own validity.  
* Implementation: 1\. Native Handle: Created buffers are inserted into the task's HandleTable, returning a File Descriptor (FD). This enables RAII and native Scarlet API usage.  
  2\. Session Reference: The ABI layer (DrmFile) holds a strong reference (Arc\<KernelObject\>) to the buffer.  
* **Benefit:** Prevents "use-after-free" or "wrong-object" errors if the DrmFile is shared between tasks that have different HandleTable states.

### **2.2 Separation of Concerns**

* **Core (Scarlet):** Manages the actual video memory, buffer allocation, and hardware abstraction.  
* **ABI (Linux DRM):** Acts as a translation layer. It maps Linux-specific "GEM Handles" (local u32 IDs) to Scarlet's "Kernel Objects".

### **2.3 Control Operations**

* **Principle:** Buffers should be controllable entities.  
* **Implementation:** GraphicsBuffer implements ControlOps.  
* **Benefit:** Allows ioctl operations directly on buffer handles (enabling future DMA-BUF synchronization primitives and cache control).

## **3\. Architecture Diagram**

classDiagram  
    %% User Space Layer  
    class UserSpaceApp {  
        \+int drm\_fd (e.g., 3\)  
        \+u32 gem\_handle (e.g., 1\)  
        ioctl(drm\_fd, ...)  
    }

    %% Kernel Core Layer  
    class Task {  
        \+HandleTable handles  
    }

    class HandleTable {  
        \+get(handle) \-\> KernelObject  
        \+insert(KernelObject) \-\> Handle  
        \+remove(handle)  
    }

    class KernelObject {  
        \<\<Enumeration\>\>  
        File(Arc\<FileObject\>)  
        GraphicsBuffer(Arc\<GraphicsBuffer\>)  
    }

    %% ABI Layer (Adapter)  
    class DrmFile {  
        \<\<FileObject\>\>  
        \-device\_id: usize  
        \-gem\_handles: HashMap\<u32, Arc\~KernelObject\~\>  
        \-next\_gem\_id: u32  
        \+ioctl()  
    }

    %% Driver Layer  
    class GraphicsBufferImpl {  
        \<\<Trait Implementation\>\>  
        \+size()  
        \+physical\_address()  
        \+mmap()  
        \+control()  
    }

    UserSpaceApp \--\> Task : runs in  
    Task \--\> HandleTable : owns  
    HandleTable \--\> KernelObject : manages  
      
    %% Reference Flow  
    KernelObject \--|\> DrmFile : variant File (FD 3\)  
    KernelObject \--|\> GraphicsBufferImpl : variant GraphicsBuffer (FD 4\)  
      
    DrmFile \--\> GraphicsBufferImpl : Internal Strong Ref (Arc)  
    DrmFile ..\> UserSpaceApp : Maps GEM(1) \-\> Arc\<Buffer\>

## **4\. Component Design**

### **4.1 Core Layer: GraphicsBuffer Trait**

A new trait representing a contiguous region of graphics memory (VRAM or GTT).

// kernel/src/device/graphics/buffer.rs

pub trait GraphicsBuffer: Send \+ Sync \+ MemoryMappingOps \+ ControlOps {  
    /// Get the size of the buffer in bytes  
    fn size(\&self) \-\> usize;  
      
    /// Get the physical address (if applicable/visible to CPU)  
    fn physical\_address(\&self) \-\> usize;  
}

### **4.2 Object Layer: KernelObject Integration**

Add GraphicsBuffer as a new variant to the unified KernelObject enum.

// kernel/src/object/mod.rs

pub enum KernelObject {  
    File(Arc\<dyn FileObject\>),  
    Pipe(Arc\<dyn PipeObject\>),  
    // ...  
    GraphicsBuffer(Arc\<dyn GraphicsBuffer\>), // New  
}

// Capability delegation  
impl KernelObject {  
    pub fn as\_control(\&self) \-\> Option\<\&dyn ControlOps\> {  
        match self {  
            // ...  
            KernelObject::GraphicsBuffer(b) \=\> Some(b.as\_ref()),  
            // ...  
        }  
    }  
}

### **4.3 ABI Layer: DrmFile**

The DrmFile structure replaces the previous DrmDeviceContext. It implements FileObject and manages the mapping between Linux concepts and Scarlet objects using strong references.

// kernel/src/abi/linux/drm/file.rs

pub struct DrmFile {  
    /// Connection to the physical device (Session)  
    device\_id: usize,  
      
    /// Translation Table: Linux GEM Handle \-\> Scarlet Object Entity  
    /// We store Arc\<KernelObject\> instead of Handle(usize) to ensure  
    /// safety when DrmFile is shared across tasks (e.g., via fork/IPC).  
    gem\_handles: Mutex\<HashMap\<u32, Arc\<KernelObject\>\>\>,  
    next\_gem\_id: Mutex\<u32\>,  
}

## **5\. Interaction Workflows**

### **5.1 Creating a Dumb Buffer (DRM\_IOCTL\_MODE\_CREATE\_DUMB)**

1. **Core Request:** DrmFile requests GraphicsManager to allocate a buffer.  
   * let buffer \= GraphicsManager::create\_buffer(width, height, format)?;  
   * let buffer\_obj \= Arc::new(KernelObject::GraphicsBuffer(buffer));  
2. **Kernel Registration (Native):** The buffer is inserted into the current task's HandleTable for native access/RAII.  
   * let scarlet\_handle \= current\_task().handle\_table.insert(buffer\_obj.clone())?;  
3. **ABI Registration (Session):** DrmFile stores the Arc in its internal map.  
   * gem\_handles.insert(gem\_id, buffer\_obj);  
4. **Response:** Returns handle \= gem\_id (u32) to the user application.

### **5.2 Page Flipping (DRM\_IOCTL\_MODE\_PAGE\_FLIP)**

1. **Input:** User passes gem\_handle \= 1\.  
2. **Lookup:** DrmFile looks up gem\_handle in its map \-\> finds Arc\<KernelObject\>.  
   * *Note: It does NOT look up via HandleTable. This ensures that even if the task context changes, DrmFile refers to the correct object.*  
3. **Validation:** Confirms it is a KernelObject::GraphicsBuffer.  
4. **Execution:** Calls GraphicsManager::flush\_buffer(device\_id, buffer) (or similar).

### **5.3 Resource Cleanup (Closing DRM FD)**

When the application closes the DRM file descriptor (or crashes):

1. DrmFile::drop() is called.  
2. It drops the gem\_handles map.  
3. This decrements the reference counts (Arc) of all held GraphicsBuffer objects.  
4. If the Task also terminates, its HandleTable is destroyed, releasing the other Arc references.  
5. When all references are gone, the memory is automatically freed.

## **6\. Future Extensibility**

* **DMA-BUF Support:** Since GraphicsBuffer is a KernelObject and implements ControlOps, exporting it as a file descriptor (DMA-BUF) is trivial (it already *is* one).  
* **Native GUI Apps:** Scarlet native applications can skip the DRM layer and use GraphicsBuffer handles directly for higher performance.  
* **IPC:** Handles can be passed between processes using standard IPC mechanisms, allowing zero-copy buffer sharing between a compositor and clients.