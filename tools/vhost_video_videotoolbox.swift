#!/usr/bin/env swift
import CoreMedia
import CoreVideo
import Darwin
import Foundation
import VideoToolbox

private let vhostUserGetFeatures: UInt32 = 1
private let vhostUserSetFeatures: UInt32 = 2
private let vhostUserSetOwner: UInt32 = 3
private let vhostUserResetOwner: UInt32 = 4
private let vhostUserSetMemTable: UInt32 = 5
private let vhostUserSetVringNum: UInt32 = 8
private let vhostUserSetVringAddr: UInt32 = 9
private let vhostUserSetVringBase: UInt32 = 10
private let vhostUserGetVringBase: UInt32 = 11
private let vhostUserSetVringKick: UInt32 = 12
private let vhostUserSetVringCall: UInt32 = 13
private let vhostUserSetVringErr: UInt32 = 14
private let vhostUserGetProtocolFeatures: UInt32 = 15
private let vhostUserSetProtocolFeatures: UInt32 = 16
private let vhostUserGetQueueNum: UInt32 = 17
private let vhostUserSetVringEnable: UInt32 = 18
private let vhostUserGetConfig: UInt32 = 24
private let vhostUserSetConfig: UInt32 = 25
private let vhostUserResetDevice: UInt32 = 34
private let vhostUserSetStatus: UInt32 = 39
private let vhostUserGetStatus: UInt32 = 40

private let vhostUserVersion: UInt32 = 0x1
private let vhostUserReply: UInt32 = 0x4
private let vhostUserNeedReply: UInt32 = 0x8

private let vhostUserFProtocolFeatures: UInt64 = 30
private let virtioFVersion1: UInt64 = 32
private let vhostUserProtocolFMq: UInt64 = 0
private let vhostUserProtocolFConfig: UInt64 = 9

private let virtioVideoFResourceGuestPages: UInt64 = 0
private let virtioVideoCmdQueryCapability: UInt32 = 256
private let virtioVideoCmdStreamCreate: UInt32 = 257
private let virtioVideoCmdResourceCreate: UInt32 = 260
private let virtioVideoCmdResourceQueue: UInt32 = 261
private let virtioVideoCmdResourceDestroyAll: UInt32 = 262
private let virtioVideoRespOkNoData: UInt32 = 512
private let virtioVideoRespOkQueryCapability: UInt32 = 513
private let virtioVideoRespOkResourceQueue: UInt32 = 514
private let virtioVideoRespErrInvalidOperation: UInt32 = 768
private let virtioVideoRespErrInvalidParameter: UInt32 = 772
private let virtioVideoQueueTypeInput: UInt32 = 256
private let virtioVideoQueueTypeOutput: UInt32 = 257
private let virtioVideoPlanesLayoutSingleBuffer: UInt32 = 1
private let virtioVideoFormatNv12: UInt32 = 3
private let virtioVideoFormatH264: UInt32 = 4098
private let virtioVideoFormatAV1: UInt32 = 4103
private let virtioVideoMemTypeGuestPages: UInt32 = 0

private let scarletVideoFrameMagic = Array("SVF1".utf8)
private let scarletAV1AccessUnitMagic = Array("SVA1".utf8)
private let nv12PixelFormat: UInt32 = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange

private extension Data {
    func u16(_ offset: Int) -> UInt16 {
        UInt16(self[offset]) | (UInt16(self[offset + 1]) << 8)
    }

    func u32(_ offset: Int) -> UInt32 {
        UInt32(self[offset])
            | (UInt32(self[offset + 1]) << 8)
            | (UInt32(self[offset + 2]) << 16)
            | (UInt32(self[offset + 3]) << 24)
    }

    func u64(_ offset: Int) -> UInt64 {
        UInt64(u32(offset)) | (UInt64(u32(offset + 4)) << 32)
    }
}

private func appendU32(_ data: inout Data, _ value: UInt32) {
    var little = value.littleEndian
    withUnsafeBytes(of: &little) { data.append(contentsOf: $0) }
}

private func appendU64(_ data: inout Data, _ value: UInt64) {
    var little = value.littleEndian
    withUnsafeBytes(of: &little) { data.append(contentsOf: $0) }
}

private func readFully(_ fd: Int32, count: Int) throws -> Data {
    var data = Data(count: count)
    var offset = 0
    while offset < count {
        let n = data.withUnsafeMutableBytes { raw -> Int in
            guard let base = raw.baseAddress else { return -1 }
            return Darwin.read(fd, base.advanced(by: offset), count - offset)
        }
        if n == 0 {
            throw RuntimeError("connection closed while reading payload")
        }
        if n < 0 {
            if errno == EINTR { continue }
            throw RuntimeError("read failed: \(String(cString: strerror(errno)))")
        }
        offset += n
    }
    return data
}

private func writeFully(_ fd: Int32, _ data: Data) throws {
    try data.withUnsafeBytes { raw in
        guard let base = raw.baseAddress else { return }
        var offset = 0
        while offset < data.count {
            let n = Darwin.write(fd, base.advanced(by: offset), data.count - offset)
            if n < 0 {
                if errno == EINTR { continue }
                throw RuntimeError("write failed: \(String(cString: strerror(errno)))")
            }
            offset += n
        }
    }
}

private struct RuntimeError: Error, CustomStringConvertible {
    let description: String

    init(_ description: String) {
        self.description = description
    }
}

private func log(_ message: String) {
    fputs(message + "\n", stdout)
    fflush(stdout)
}

private final class MemoryRegion {
    let guestAddress: UInt64
    let size: UInt64
    let userAddress: UInt64
    let mmapOffset: UInt64
    let fd: Int32
    let mapping: UnsafeMutableRawPointer
    let mappingLength: Int

    init(guestAddress: UInt64, size: UInt64, userAddress: UInt64, mmapOffset: UInt64, fd: Int32) throws {
        self.guestAddress = guestAddress
        self.size = size
        self.userAddress = userAddress
        self.mmapOffset = mmapOffset
        self.fd = fd
        self.mappingLength = Int(mmapOffset + size)
        let pointer = mmap(nil, mappingLength, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0)
        if pointer == MAP_FAILED {
            throw RuntimeError("mmap failed: \(String(cString: strerror(errno)))")
        }
        self.mapping = pointer!
    }

    deinit {
        munmap(mapping, mappingLength)
        close(fd)
    }
}

private final class GuestMemory {
    private var regions: [MemoryRegion] = []

    func reset() {
        regions.removeAll()
    }

    func setRegions(payload: Data, fds: [Int32]) throws {
        reset()
        guard payload.count >= 8 else { throw RuntimeError("short SET_MEM_TABLE payload") }
        let regionCount = Int(payload.u32(0))
        guard regionCount == fds.count else {
            throw RuntimeError("SET_MEM_TABLE regions=\(regionCount) fds=\(fds.count)")
        }

        var offset = 8
        for index in 0..<regionCount {
            let guestAddress = payload.u64(offset)
            let size = payload.u64(offset + 8)
            let userAddress = payload.u64(offset + 16)
            let mmapOffset = payload.u64(offset + 24)
            offset += 32
            regions.append(try MemoryRegion(
                guestAddress: guestAddress,
                size: size,
                userAddress: userAddress,
                mmapOffset: mmapOffset,
                fd: fds[index]
            ))
        }
        log("[vhost-video-vt] mapped \(regionCount) memory regions")
    }

    private func translate(_ address: UInt64, _ length: Int, user: Bool) throws -> (MemoryRegion, Int) {
        let end = address + UInt64(length)
        for region in regions {
            let base = user ? region.userAddress : region.guestAddress
            if base <= address && end <= base + region.size {
                return (region, Int(region.mmapOffset + (address - base)))
            }
        }
        throw RuntimeError("address 0x\(String(address, radix: 16))+0x\(String(length, radix: 16)) is not mapped")
    }

    func readUser(_ address: UInt64, _ length: Int) throws -> Data {
        let (region, offset) = try translate(address, length, user: true)
        return Data(bytes: region.mapping.advanced(by: offset), count: length)
    }

    func writeUser(_ address: UInt64, _ data: Data) throws {
        let (region, offset) = try translate(address, data.count, user: true)
        data.copyBytes(to: region.mapping.advanced(by: offset).assumingMemoryBound(to: UInt8.self), count: data.count)
    }

    func readGuest(_ address: UInt64, _ length: Int) throws -> Data {
        let (region, offset) = try translate(address, length, user: false)
        return Data(bytes: region.mapping.advanced(by: offset), count: length)
    }

    func writeGuest(_ address: UInt64, _ data: Data) throws {
        let (region, offset) = try translate(address, data.count, user: false)
        data.copyBytes(to: region.mapping.advanced(by: offset).assumingMemoryBound(to: UInt8.self), count: data.count)
    }

    func readU16User(_ address: UInt64) throws -> UInt16 {
        try readUser(address, 2).u16(0)
    }

    func writeU16User(_ address: UInt64, _ value: UInt16) throws {
        var data = Data()
        var little = value.littleEndian
        withUnsafeBytes(of: &little) { data.append(contentsOf: $0) }
        try writeUser(address, data)
    }
}

private final class VirtQueue {
    let index: Int
    var size: UInt32 = 0
    var descAddress: UInt64 = 0
    var usedAddress: UInt64 = 0
    var availAddress: UInt64 = 0
    var lastAvailIndex: UInt16 = 0
    var kickFd: Int32?
    var callFd: Int32?
    var enabled = false

    init(index: Int) {
        self.index = index
    }

    deinit {
        if let fd = kickFd { close(fd) }
        if let fd = callFd { close(fd) }
    }
}

private struct Resource {
    let queueType: UInt32
    let resourceId: UInt32
    let entries: [(address: UInt64, length: UInt32)]
}

private struct DecodedFrame {
    let width: UInt32
    let height: UInt32
    let nv12: Data
}

private protocol VideoToolboxDecoderCallback: AnyObject {
    func complete(status: OSStatus, imageBuffer: CVImageBuffer?)
}

private let decompressionCallback: VTDecompressionOutputCallback = { refCon, _, status, _, imageBuffer, _, _ in
    guard let refCon else { return }
    let decoder = Unmanaged<AnyObject>.fromOpaque(refCon).takeUnretainedValue()
    (decoder as? VideoToolboxDecoderCallback)?.complete(status: status, imageBuffer: imageBuffer)
}

private final class H264VideoToolboxDecoder: VideoToolboxDecoderCallback {
    private var sps: Data?
    private var pps: Data?
    private var formatDescription: CMVideoFormatDescription?
    private var session: VTDecompressionSession?
    private let lock = NSLock()
    private var pendingFrame: DecodedFrame?
    private var pendingStatus: OSStatus = noErr

    deinit {
        if let session {
            VTDecompressionSessionInvalidate(session)
        }
    }

    func complete(status: OSStatus, imageBuffer: CVImageBuffer?) {
        lock.lock()
        defer { lock.unlock() }
        pendingStatus = status
        if status == noErr, let pixelBuffer = imageBuffer {
            pendingFrame = copyNV12(pixelBuffer)
        }
    }

    func decode(_ annexB: Data) throws -> DecodedFrame {
        let units = nalUnits(from: annexB)
        guard !units.isEmpty else { throw RuntimeError("H.264 input contains no NAL units") }

        var accessUnit = Data()
        var sawVcl = false
        var sawIdr = false
        var parameterSetChanged = false
        for unit in units {
            guard let first = unit.first else { continue }
            let nalType = first & 0x1f
            if nalType == 7 {
                if sps != unit { parameterSetChanged = true }
                sps = unit
                continue
            } else if nalType == 8 {
                if pps != unit { parameterSetChanged = true }
                pps = unit
                continue
            } else if nalType == 9 {
                continue
            }
            if (1...5).contains(nalType) {
                sawVcl = true
            }
            if nalType == 5 {
                sawIdr = true
            }
            appendLengthPrefixedNal(unit, to: &accessUnit)
        }

        guard sawVcl else { throw RuntimeError("H.264 input did not contain a VCL NAL") }
        if parameterSetChanged || session == nil {
            try rebuildSession()
        }
        guard let session, let formatDescription else {
            throw RuntimeError("VideoToolbox session is not ready; SPS/PPS required")
        }

        var blockBuffer: CMBlockBuffer?
        var status = CMBlockBufferCreateWithMemoryBlock(
            allocator: kCFAllocatorDefault,
            memoryBlock: nil,
            blockLength: accessUnit.count,
            blockAllocator: kCFAllocatorDefault,
            customBlockSource: nil,
            offsetToData: 0,
            dataLength: accessUnit.count,
            flags: 0,
            blockBufferOut: &blockBuffer
        )
        guard status == noErr, let blockBuffer else {
            throw RuntimeError("CMBlockBufferCreateWithMemoryBlock failed: \(status)")
        }
        try accessUnit.withUnsafeBytes { raw in
            guard let base = raw.baseAddress else { return }
            let replaceStatus = CMBlockBufferReplaceDataBytes(with: base, blockBuffer: blockBuffer, offsetIntoDestination: 0, dataLength: accessUnit.count)
            if replaceStatus != noErr {
                throw RuntimeError("CMBlockBufferReplaceDataBytes failed: \(replaceStatus)")
            }
        }

        var sampleBuffer: CMSampleBuffer?
        var sampleSize = accessUnit.count
        status = CMSampleBufferCreateReady(
            allocator: kCFAllocatorDefault,
            dataBuffer: blockBuffer,
            formatDescription: formatDescription,
            sampleCount: 1,
            sampleTimingEntryCount: 0,
            sampleTimingArray: nil,
            sampleSizeEntryCount: 1,
            sampleSizeArray: &sampleSize,
            sampleBufferOut: &sampleBuffer
        )
        guard status == noErr, let sampleBuffer else {
            throw RuntimeError("CMSampleBufferCreateReady failed: \(status)")
        }
        setSampleAttachments(sampleBuffer, isSync: sawIdr)

        lock.lock()
        pendingFrame = nil
        pendingStatus = noErr
        lock.unlock()

        var infoFlags = VTDecodeInfoFlags()
        status = VTDecompressionSessionDecodeFrame(
            session,
            sampleBuffer: sampleBuffer,
            flags: [],
            frameRefcon: nil,
            infoFlagsOut: &infoFlags
        )
        guard status == noErr else {
            throw RuntimeError("VTDecompressionSessionDecodeFrame failed: \(status)")
        }
        VTDecompressionSessionWaitForAsynchronousFrames(session)

        lock.lock()
        defer { lock.unlock() }
        guard pendingStatus == noErr else {
            throw RuntimeError("VideoToolbox callback failed: \(pendingStatus)")
        }
        guard let frame = pendingFrame else {
            throw RuntimeError("VideoToolbox produced no frame")
        }
        return frame
    }

    private func setSampleAttachments(_ sampleBuffer: CMSampleBuffer, isSync: Bool) {
        guard let attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, createIfNecessary: true),
              CFArrayGetCount(attachments) > 0,
              let attachment = CFArrayGetValueAtIndex(attachments, 0)
        else {
            return
        }

        let dictionary = unsafeBitCast(attachment, to: CFMutableDictionary.self)
        CFDictionarySetValue(
            dictionary,
            Unmanaged.passUnretained(kCMSampleAttachmentKey_DisplayImmediately).toOpaque(),
            Unmanaged.passUnretained(kCFBooleanTrue).toOpaque()
        )
        if !isSync {
            CFDictionarySetValue(
                dictionary,
                Unmanaged.passUnretained(kCMSampleAttachmentKey_NotSync).toOpaque(),
                Unmanaged.passUnretained(kCFBooleanTrue).toOpaque()
            )
        }
    }

    private func rebuildSession() throws {
        guard let sps, let pps else {
            throw RuntimeError("SPS/PPS missing")
        }

        if let session {
            VTDecompressionSessionInvalidate(session)
        }
        session = nil

        var newDescription: CMVideoFormatDescription?
        try sps.withUnsafeBytes { spsRaw in
            try pps.withUnsafeBytes { ppsRaw in
                guard
                    let spsBase = spsRaw.bindMemory(to: UInt8.self).baseAddress,
                    let ppsBase = ppsRaw.bindMemory(to: UInt8.self).baseAddress
                else {
                    throw RuntimeError("empty parameter set")
                }
                var pointers: [UnsafePointer<UInt8>] = [spsBase, ppsBase]
                var sizes = [sps.count, pps.count]
                let status = CMVideoFormatDescriptionCreateFromH264ParameterSets(
                    allocator: kCFAllocatorDefault,
                    parameterSetCount: 2,
                    parameterSetPointers: &pointers,
                    parameterSetSizes: &sizes,
                    nalUnitHeaderLength: 4,
                    formatDescriptionOut: &newDescription
                )
                if status != noErr {
                    throw RuntimeError("CMVideoFormatDescriptionCreateFromH264ParameterSets failed: \(status)")
                }
            }
        }
        guard let newDescription else { throw RuntimeError("missing format description") }
        formatDescription = newDescription

        var callback = VTDecompressionOutputCallbackRecord(
            decompressionOutputCallback: decompressionCallback,
            decompressionOutputRefCon: Unmanaged.passUnretained(self).toOpaque()
        )
        let decoderSpec = [
            kVTVideoDecoderSpecification_RequireHardwareAcceleratedVideoDecoder as String: true
        ] as CFDictionary
        let imageAttributes = [
            kCVPixelBufferPixelFormatTypeKey as String: nv12PixelFormat,
            kCVPixelBufferIOSurfacePropertiesKey as String: [:]
        ] as CFDictionary

        var newSession: VTDecompressionSession?
        let status = VTDecompressionSessionCreate(
            allocator: kCFAllocatorDefault,
            formatDescription: newDescription,
            decoderSpecification: decoderSpec,
            imageBufferAttributes: imageAttributes,
            outputCallback: &callback,
            decompressionSessionOut: &newSession
        )
        guard status == noErr, let newSession else {
            throw RuntimeError("VTDecompressionSessionCreate hardware decoder failed: \(status)")
        }
        session = newSession

        let dimensions = CMVideoFormatDescriptionGetDimensions(newDescription)
        log("[vhost-video-vt] VideoToolbox H.264 hardware session \(dimensions.width)x\(dimensions.height)")
    }

    private func copyNV12(_ pixelBuffer: CVImageBuffer) -> DecodedFrame? {
        let pixelBuffer = pixelBuffer as CVPixelBuffer
        CVPixelBufferLockBaseAddress(pixelBuffer, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(pixelBuffer, .readOnly) }

        guard CVPixelBufferGetPlaneCount(pixelBuffer) >= 2 else { return nil }
        let width = CVPixelBufferGetWidthOfPlane(pixelBuffer, 0)
        let height = CVPixelBufferGetHeightOfPlane(pixelBuffer, 0)
        let yStride = CVPixelBufferGetBytesPerRowOfPlane(pixelBuffer, 0)
        let uvStride = CVPixelBufferGetBytesPerRowOfPlane(pixelBuffer, 1)
        guard
            let yBase = CVPixelBufferGetBaseAddressOfPlane(pixelBuffer, 0),
            let uvBase = CVPixelBufferGetBaseAddressOfPlane(pixelBuffer, 1)
        else {
            return nil
        }

        var data = Data()
        data.reserveCapacity(width * height * 3 / 2)
        for row in 0..<height {
            data.append(yBase.advanced(by: row * yStride).assumingMemoryBound(to: UInt8.self), count: width)
        }
        for row in 0..<(height / 2) {
            data.append(uvBase.advanced(by: row * uvStride).assumingMemoryBound(to: UInt8.self), count: width)
        }
        return DecodedFrame(width: UInt32(width), height: UInt32(height), nv12: data)
    }

    private func appendLengthPrefixedNal(_ nal: Data, to out: inout Data) {
        let length = UInt32(nal.count).bigEndian
        var big = length
        withUnsafeBytes(of: &big) { out.append(contentsOf: $0) }
        out.append(nal)
    }

    private func nalUnits(from data: Data) -> [Data] {
        let bytes = [UInt8](data)
        var starts: [(offset: Int, codeLength: Int)] = []
        var index = 0
        while index + 3 < bytes.count {
            if bytes[index] == 0 && bytes[index + 1] == 0 && bytes[index + 2] == 1 {
                starts.append((index, 3))
                index += 3
            } else if index + 4 < bytes.count
                && bytes[index] == 0
                && bytes[index + 1] == 0
                && bytes[index + 2] == 0
                && bytes[index + 3] == 1 {
                starts.append((index, 4))
                index += 4
            } else {
                index += 1
            }
        }
        if starts.isEmpty {
            return [data]
        }

        var units: [Data] = []
        for i in 0..<starts.count {
            let start = starts[i].offset + starts[i].codeLength
            let end = i + 1 < starts.count ? starts[i + 1].offset : bytes.count
            if start < end {
                units.append(Data(bytes[start..<end]))
            }
        }
        return units
    }
}

private final class AV1VideoToolboxDecoder: VideoToolboxDecoderCallback {
    private var configRecord: Data?
    private var width: UInt32 = 0
    private var height: UInt32 = 0
    private var formatDescription: CMVideoFormatDescription?
    private var session: VTDecompressionSession?
    private let lock = NSLock()
    private var pendingFrame: DecodedFrame?
    private var pendingStatus: OSStatus = noErr

    deinit {
        if let session {
            VTDecompressionSessionInvalidate(session)
        }
    }

    func complete(status: OSStatus, imageBuffer: CVImageBuffer?) {
        lock.lock()
        defer { lock.unlock() }
        pendingStatus = status
        if status == noErr, let pixelBuffer = imageBuffer {
            pendingFrame = copyNV12(pixelBuffer)
        }
    }

    func decode(_ packet: Data) throws -> DecodedFrame {
        guard packet.count >= 20,
              Array(packet[0..<4]) == scarletAV1AccessUnitMagic
        else {
            throw RuntimeError("AV1 input missing Scarlet av1C header")
        }
        let packetWidth = packet.u32(4)
        let packetHeight = packet.u32(8)
        let configLength = Int(packet.u32(12))
        let sampleLength = Int(packet.u32(16))
        let configStart = 20
        let sampleStart = configStart + configLength
        let packetEnd = sampleStart + sampleLength
        guard packetWidth > 0, packetHeight > 0, configLength >= 4, packetEnd <= packet.count else {
            throw RuntimeError("AV1 input header is invalid")
        }

        let packetConfig = packet.subdata(in: configStart..<sampleStart)
        if session == nil || configRecord != packetConfig || width != packetWidth || height != packetHeight {
            configRecord = packetConfig
            width = packetWidth
            height = packetHeight
            try rebuildSession()
        }
        guard let session, let formatDescription else {
            throw RuntimeError("VideoToolbox AV1 session is not ready")
        }

        let sample = packet.subdata(in: sampleStart..<packetEnd)
        var blockBuffer: CMBlockBuffer?
        var status = CMBlockBufferCreateWithMemoryBlock(
            allocator: kCFAllocatorDefault,
            memoryBlock: nil,
            blockLength: sample.count,
            blockAllocator: kCFAllocatorDefault,
            customBlockSource: nil,
            offsetToData: 0,
            dataLength: sample.count,
            flags: 0,
            blockBufferOut: &blockBuffer
        )
        guard status == noErr, let blockBuffer else {
            throw RuntimeError("CMBlockBufferCreateWithMemoryBlock failed: \(status)")
        }
        try sample.withUnsafeBytes { raw in
            guard let base = raw.baseAddress else { return }
            let replaceStatus = CMBlockBufferReplaceDataBytes(with: base, blockBuffer: blockBuffer, offsetIntoDestination: 0, dataLength: sample.count)
            if replaceStatus != noErr {
                throw RuntimeError("CMBlockBufferReplaceDataBytes failed: \(replaceStatus)")
            }
        }

        var sampleBuffer: CMSampleBuffer?
        var sampleSize = sample.count
        status = CMSampleBufferCreateReady(
            allocator: kCFAllocatorDefault,
            dataBuffer: blockBuffer,
            formatDescription: formatDescription,
            sampleCount: 1,
            sampleTimingEntryCount: 0,
            sampleTimingArray: nil,
            sampleSizeEntryCount: 1,
            sampleSizeArray: &sampleSize,
            sampleBufferOut: &sampleBuffer
        )
        guard status == noErr, let sampleBuffer else {
            throw RuntimeError("CMSampleBufferCreateReady failed: \(status)")
        }
        setSampleAttachments(sampleBuffer)

        lock.lock()
        pendingFrame = nil
        pendingStatus = noErr
        lock.unlock()

        var infoFlags = VTDecodeInfoFlags()
        status = VTDecompressionSessionDecodeFrame(
            session,
            sampleBuffer: sampleBuffer,
            flags: [],
            frameRefcon: nil,
            infoFlagsOut: &infoFlags
        )
        guard status == noErr else {
            throw RuntimeError("VTDecompressionSessionDecodeFrame failed: \(status)")
        }
        VTDecompressionSessionWaitForAsynchronousFrames(session)

        lock.lock()
        defer { lock.unlock() }
        guard pendingStatus == noErr else {
            throw RuntimeError("VideoToolbox callback failed: \(pendingStatus)")
        }
        guard let frame = pendingFrame else {
            throw RuntimeError("VideoToolbox produced no AV1 frame")
        }
        return frame
    }

    private func rebuildSession() throws {
        guard let configRecord, width > 0, height > 0 else {
            throw RuntimeError("AV1 configuration missing")
        }
        if let session {
            VTDecompressionSessionInvalidate(session)
        }
        session = nil

        var extensions: CFDictionary?
        let av1ConfigKey = "av1C" as CFString
        let extensionAtoms = [av1ConfigKey: configRecord] as CFDictionary
        extensions = [kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms as String: extensionAtoms] as CFDictionary

        var newDescription: CMVideoFormatDescription?
        let status = CMVideoFormatDescriptionCreate(
            allocator: kCFAllocatorDefault,
            codecType: kCMVideoCodecType_AV1,
            width: Int32(width),
            height: Int32(height),
            extensions: extensions,
            formatDescriptionOut: &newDescription
        )
        guard status == noErr, let newDescription else {
            throw RuntimeError("CMVideoFormatDescriptionCreate AV1 failed: \(status)")
        }
        formatDescription = newDescription

        var callback = VTDecompressionOutputCallbackRecord(
            decompressionOutputCallback: decompressionCallback,
            decompressionOutputRefCon: Unmanaged.passUnretained(self).toOpaque()
        )
        let decoderSpec = [
            kVTVideoDecoderSpecification_RequireHardwareAcceleratedVideoDecoder as String: true
        ] as CFDictionary
        let imageAttributes = [
            kCVPixelBufferPixelFormatTypeKey as String: nv12PixelFormat,
            kCVPixelBufferIOSurfacePropertiesKey as String: [:]
        ] as CFDictionary

        var newSession: VTDecompressionSession?
        let sessionStatus = VTDecompressionSessionCreate(
            allocator: kCFAllocatorDefault,
            formatDescription: newDescription,
            decoderSpecification: decoderSpec,
            imageBufferAttributes: imageAttributes,
            outputCallback: &callback,
            decompressionSessionOut: &newSession
        )
        guard sessionStatus == noErr, let newSession else {
            throw RuntimeError("VTDecompressionSessionCreate AV1 hardware decoder failed: \(sessionStatus)")
        }
        session = newSession
        log("[vhost-video-vt] VideoToolbox AV1 hardware session \(width)x\(height)")
    }

    private func setSampleAttachments(_ sampleBuffer: CMSampleBuffer) {
        guard let attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, createIfNecessary: true),
              CFArrayGetCount(attachments) > 0,
              let attachment = CFArrayGetValueAtIndex(attachments, 0)
        else {
            return
        }
        let dictionary = unsafeBitCast(attachment, to: CFMutableDictionary.self)
        CFDictionarySetValue(
            dictionary,
            Unmanaged.passUnretained(kCMSampleAttachmentKey_DisplayImmediately).toOpaque(),
            Unmanaged.passUnretained(kCFBooleanTrue).toOpaque()
        )
    }

    private func copyNV12(_ pixelBuffer: CVImageBuffer) -> DecodedFrame? {
        let pixelBuffer = pixelBuffer as CVPixelBuffer
        CVPixelBufferLockBaseAddress(pixelBuffer, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(pixelBuffer, .readOnly) }

        guard CVPixelBufferGetPlaneCount(pixelBuffer) >= 2 else { return nil }
        let width = CVPixelBufferGetWidthOfPlane(pixelBuffer, 0)
        let height = CVPixelBufferGetHeightOfPlane(pixelBuffer, 0)
        let yStride = CVPixelBufferGetBytesPerRowOfPlane(pixelBuffer, 0)
        let uvStride = CVPixelBufferGetBytesPerRowOfPlane(pixelBuffer, 1)
        guard
            let yBase = CVPixelBufferGetBaseAddressOfPlane(pixelBuffer, 0),
            let uvBase = CVPixelBufferGetBaseAddressOfPlane(pixelBuffer, 1)
        else {
            return nil
        }

        var data = Data()
        data.reserveCapacity(width * height * 3 / 2)
        for row in 0..<height {
            data.append(yBase.advanced(by: row * yStride).assumingMemoryBound(to: UInt8.self), count: width)
        }
        for row in 0..<(height / 2) {
            data.append(uvBase.advanced(by: row * uvStride).assumingMemoryBound(to: UInt8.self), count: width)
        }
        return DecodedFrame(width: UInt32(width), height: UInt32(height), nv12: data)
    }
}

private final class VideoBackend {
    let memory = GuestMemory()
    var queues: [VirtQueue]
    private var streams = Set<UInt32>()
    private var streamFormats: [UInt32: UInt32] = [:]
    private var resources: [String: Resource] = [:]
    private var queuedOutputResources: [UInt32: UInt32] = [:]
    private var h264Decoders: [UInt32: H264VideoToolboxDecoder] = [:]
    private var av1Decoders: [UInt32: AV1VideoToolboxDecoder] = [:]

    init(queueCount: Int) {
        queues = (0..<queueCount).map { VirtQueue(index: $0) }
    }

    func queue(_ index: Int) throws -> VirtQueue {
        guard index < queues.count else { throw RuntimeError("invalid queue index \(index)") }
        return queues[index]
    }

    func processKick(queueIndex: Int) throws {
        let queue = try queue(queueIndex)
        if let kickFd = queue.kickFd {
            var buf: UInt64 = 0
            _ = withUnsafeMutableBytes(of: &buf) { Darwin.read(kickFd, $0.baseAddress, 8) }
        }

        let availIndex = try memory.readU16User(queue.availAddress + 2)
        while queue.lastAvailIndex != availIndex {
            let ringOffset = UInt64(4 + 2 * (Int(queue.lastAvailIndex) % Int(queue.size)))
            let head = try memory.readU16User(queue.availAddress + ringOffset)
            try processDescriptorChain(queue: queue, head: head)
            queue.lastAvailIndex &+= 1
        }
    }

    private func readDescriptor(queue: VirtQueue, index: UInt16) throws -> (address: UInt64, length: UInt32, flags: UInt16, next: UInt16) {
        let raw = try memory.readUser(queue.descAddress + UInt64(index) * 16, 16)
        return (raw.u64(0), raw.u32(8), raw.u16(12), raw.u16(14))
    }

    private func processDescriptorChain(queue: VirtQueue, head: UInt16) throws {
        let requestDesc = try readDescriptor(queue: queue, index: head)
        guard requestDesc.flags & 0x1 != 0 else {
            try pushUsed(queue: queue, head: head, length: 0)
            return
        }
        let responseDesc = try readDescriptor(queue: queue, index: requestDesc.next)
        guard responseDesc.flags & 0x2 != 0 else {
            try pushUsed(queue: queue, head: head, length: 0)
            return
        }

        let request = try memory.readGuest(requestDesc.address, Int(requestDesc.length))
        var response = makeResponse(request)
        if response.count > Int(responseDesc.length) {
            response = response.prefix(Int(responseDesc.length))
        }
        if response.count < Int(responseDesc.length) {
            response.append(Data(count: Int(responseDesc.length) - response.count))
        }
        try memory.writeGuest(responseDesc.address, response)
        try pushUsed(queue: queue, head: head, length: UInt32(response.count))
    }

    private func pushUsed(queue: VirtQueue, head: UInt16, length: UInt32) throws {
        let usedIndex = try memory.readU16User(queue.usedAddress + 2)
        var entry = Data()
        appendU32(&entry, UInt32(head))
        appendU32(&entry, length)
        let ringOffset = UInt64(4 + 8 * (Int(usedIndex) % Int(queue.size)))
        try memory.writeUser(queue.usedAddress + ringOffset, entry)
        try memory.writeU16User(queue.usedAddress + 2, usedIndex &+ 1)
        if let callFd = queue.callFd {
            var one: UInt64 = 1
            _ = withUnsafeBytes(of: &one) { Darwin.write(callFd, $0.baseAddress, 8) }
        }
    }

    private func makeResponse(_ request: Data) -> Data {
        guard request.count >= 8 else {
            return header(virtioVideoRespErrInvalidOperation, streamId: 0)
        }
        let command = request.u32(0)
        let streamId = request.u32(4)

        do {
            switch command {
            case virtioVideoCmdQueryCapability:
                return try queryCapability(request: request, streamId: streamId)
            case virtioVideoCmdStreamCreate:
                return try streamCreate(request: request, streamId: streamId)
            case virtioVideoCmdResourceCreate:
                return try resourceCreate(request: request, streamId: streamId)
            case virtioVideoCmdResourceQueue:
                return try resourceQueue(request: request, streamId: streamId)
            case virtioVideoCmdResourceDestroyAll:
                return try resourceDestroyAll(request: request, streamId: streamId)
            default:
                log("[vhost-video-vt] unsupported virtio-video command \(command)")
                return header(virtioVideoRespErrInvalidOperation, streamId: streamId)
            }
        } catch {
            log("[vhost-video-vt] command \(command) failed: \(error)")
            return header(virtioVideoRespErrInvalidOperation, streamId: streamId)
        }
    }

    private func queryCapability(request: Data, streamId: UInt32) throws -> Data {
        guard request.count >= 12 else { throw RuntimeError("short QUERY_CAPABILITY") }
        let queueType = request.u32(8)
        let format: UInt32
        if queueType == virtioVideoQueueTypeInput {
            format = virtioVideoFormatH264
        } else if queueType == virtioVideoQueueTypeOutput {
            format = virtioVideoFormatNv12
        } else {
            return header(virtioVideoRespErrInvalidParameter, streamId: streamId)
        }

        log("[vhost-video-vt] QUERY_CAPABILITY queue_type=\(queueType) format=\(format)")
        var response = header(virtioVideoRespOkQueryCapability, streamId: streamId)
        appendU32(&response, queueType == virtioVideoQueueTypeInput ? 2 : 1)
        appendU32(&response, 0)
        appendU64(&response, 0)
        appendU32(&response, format)
        appendU32(&response, virtioVideoPlanesLayoutSingleBuffer)
        appendU32(&response, 4096)
        appendU32(&response, 0)
        if queueType == virtioVideoQueueTypeInput {
            appendU64(&response, 0)
            appendU32(&response, virtioVideoFormatAV1)
            appendU32(&response, virtioVideoPlanesLayoutSingleBuffer)
            appendU32(&response, 4096)
            appendU32(&response, 0)
        }
        return response
    }

    private func streamCreate(request: Data, streamId: UInt32) throws -> Data {
        guard request.count >= 88 else { throw RuntimeError("short STREAM_CREATE") }
        let inMem = request.u32(8)
        let outMem = request.u32(12)
        let codedFormat = request.u32(16)
        guard inMem == virtioVideoMemTypeGuestPages,
              outMem == virtioVideoMemTypeGuestPages,
              (codedFormat == virtioVideoFormatH264 || codedFormat == virtioVideoFormatAV1)
        else {
            return header(virtioVideoRespErrInvalidParameter, streamId: streamId)
        }
        streams.insert(streamId)
        streamFormats[streamId] = codedFormat
        h264Decoders[streamId] = nil
        av1Decoders[streamId] = nil
        let formatName = codedFormat == virtioVideoFormatAV1 ? "AV1" : "H264"
        log("[vhost-video-vt] STREAM_CREATE stream_id=\(streamId) format=\(formatName) hardware=required")
        return header(virtioVideoRespOkNoData, streamId: streamId)
    }

    private func resourceCreate(request: Data, streamId: UInt32) throws -> Data {
        guard streams.contains(streamId) else {
            return header(virtioVideoRespErrInvalidParameter, streamId: streamId)
        }
        guard request.count >= 104 else { throw RuntimeError("short RESOURCE_CREATE") }
        let queueType = request.u32(8)
        let resourceId = request.u32(12)
        let numPlanes = request.u32(20)
        let firstNumEntries = request.u32(56)
        guard numPlanes == 1, firstNumEntries == 1 else {
            return header(virtioVideoRespErrInvalidParameter, streamId: streamId)
        }
        let entryAddress = request.u64(88)
        let entryLength = request.u32(96)
        resources[key(streamId, queueType, resourceId)] = Resource(
            queueType: queueType,
            resourceId: resourceId,
            entries: [(entryAddress, entryLength)]
        )
        log("[vhost-video-vt] RESOURCE_CREATE stream_id=\(streamId) queue=\(queueType) resource=\(resourceId) len=\(entryLength)")
        return header(virtioVideoRespOkNoData, streamId: streamId)
    }

    private func resourceQueue(request: Data, streamId: UInt32) throws -> Data {
        guard request.count >= 64 else { throw RuntimeError("short RESOURCE_QUEUE") }
        let queueType = request.u32(8)
        let resourceId = request.u32(12)
        let timestamp = request.u64(16)
        let dataSize = request.u32(28)
        guard let resource = resources[key(streamId, queueType, resourceId)] else {
            return header(virtioVideoRespErrInvalidParameter, streamId: streamId)
        }

        if queueType == virtioVideoQueueTypeOutput {
            queuedOutputResources[streamId] = resourceId
            log("[vhost-video-vt] RESOURCE_QUEUE stream_id=\(streamId) output resource=\(resourceId) timestamp=\(timestamp)")
            return resourceQueueResponse(streamId: streamId, timestamp: timestamp, flags: 0, size: 0)
        }

        guard queueType == virtioVideoQueueTypeInput else {
            return header(virtioVideoRespErrInvalidParameter, streamId: streamId)
        }
        guard let outputId = queuedOutputResources[streamId],
              let output = resources[key(streamId, virtioVideoQueueTypeOutput, outputId)]
        else {
            throw RuntimeError("input queued without an output resource")
        }

        let input = try readResource(resource, maxLength: Int(dataSize))
        let codedFormat = streamFormats[streamId] ?? virtioVideoFormatH264
        let frame: DecodedFrame
        if codedFormat == virtioVideoFormatAV1 {
            let decoder = av1Decoders[streamId] ?? AV1VideoToolboxDecoder()
            av1Decoders[streamId] = decoder
            frame = try decoder.decode(input)
        } else {
            let decoder = h264Decoders[streamId] ?? H264VideoToolboxDecoder()
            h264Decoders[streamId] = decoder
            frame = try decoder.decode(input)
        }
        var packed = Data(scarletVideoFrameMagic)
        appendU32(&packed, frame.width)
        appendU32(&packed, frame.height)
        appendU32(&packed, nv12PixelFormat)
        appendU32(&packed, UInt32(frame.nv12.count))
        packed.append(frame.nv12)
        try writeResource(output, packed)
        queuedOutputResources[streamId] = nil

        let formatName = codedFormat == virtioVideoFormatAV1 ? "AV1" : "H264"
        log("[vhost-video-vt] decoded stream_id=\(streamId) \(formatName) timestamp=\(timestamp) \(frame.width)x\(frame.height) nv12=\(frame.nv12.count)")
        return resourceQueueResponse(streamId: streamId, timestamp: timestamp, flags: 0, size: UInt32(packed.count))
    }

    private func resourceDestroyAll(request: Data, streamId: UInt32) throws -> Data {
        guard request.count >= 16 else { throw RuntimeError("short RESOURCE_DESTROY_ALL") }
        let queueType = request.u32(8)
        resources = resources.filter { !$0.key.hasPrefix("\(streamId):\(queueType):") }
        if queueType == virtioVideoQueueTypeOutput {
            queuedOutputResources[streamId] = nil
        }
        let streamPrefix = "\(streamId):"
        if !resources.keys.contains(where: { $0.hasPrefix(streamPrefix) }) {
            queuedOutputResources[streamId] = nil
            h264Decoders[streamId] = nil
            av1Decoders[streamId] = nil
            log("[vhost-video-vt] RESOURCE_DESTROY_ALL stream_id=\(streamId) released decoder")
        }
        return header(virtioVideoRespOkNoData, streamId: streamId)
    }

    private func readResource(_ resource: Resource, maxLength: Int) throws -> Data {
        var remaining = maxLength
        var data = Data()
        for entry in resource.entries where remaining > 0 {
            let count = min(Int(entry.length), remaining)
            data.append(try memory.readGuest(entry.address, count))
            remaining -= count
        }
        return data
    }

    private func writeResource(_ resource: Resource, _ data: Data) throws {
        var offset = 0
        for entry in resource.entries where offset < data.count {
            let count = min(Int(entry.length), data.count - offset)
            try memory.writeGuest(entry.address, data.subdata(in: offset..<(offset + count)))
            offset += count
        }
        guard offset == data.count else {
            throw RuntimeError("output resource too small: wrote \(offset) of \(data.count)")
        }
    }

    private func header(_ type: UInt32, streamId: UInt32) -> Data {
        var data = Data()
        appendU32(&data, type)
        appendU32(&data, streamId)
        return data
    }

    private func resourceQueueResponse(streamId: UInt32, timestamp: UInt64, flags: UInt32, size: UInt32) -> Data {
        var data = header(virtioVideoRespOkResourceQueue, streamId: streamId)
        appendU64(&data, timestamp)
        appendU32(&data, flags)
        appendU32(&data, size)
        return data
    }

    private func key(_ streamId: UInt32, _ queueType: UInt32, _ resourceId: UInt32) -> String {
        "\(streamId):\(queueType):\(resourceId)"
    }
}

private struct VhostMessage {
    let request: UInt32
    let flags: UInt32
    let payload: Data
    let fds: [Int32]
}

private func recvMessage(_ fd: Int32) throws -> VhostMessage? {
    var header = Data(count: 12)
    var control = Data(count: 4096)
    let controlCount = control.count
    var received = 0
    var controllen = 0

    try header.withUnsafeMutableBytes { headerRaw in
        try control.withUnsafeMutableBytes { controlRaw in
            guard let headerBase = headerRaw.baseAddress, let controlBase = controlRaw.baseAddress else {
                throw RuntimeError("failed to allocate recv buffers")
            }
            var iov = iovec(iov_base: headerBase, iov_len: 12)
            try withUnsafeMutablePointer(to: &iov) { iovPointer in
                var msg = msghdr(
                    msg_name: nil,
                    msg_namelen: 0,
                    msg_iov: iovPointer,
                    msg_iovlen: 1,
                    msg_control: controlBase,
                    msg_controllen: socklen_t(controlCount),
                    msg_flags: 0
                )
                while true {
                    let n = recvmsg(fd, &msg, 0)
                    if n < 0 && errno == EINTR { continue }
                    if n < 0 {
                        throw RuntimeError("recvmsg failed: \(String(cString: strerror(errno)))")
                    }
                    received = n
                    controllen = Int(msg.msg_controllen)
                    break
                }
            }
        }
    }

    if received == 0 {
        return nil
    }
    guard received == 12 else {
        throw RuntimeError("short vhost-user header: \(received) bytes")
    }

    let request = header.u32(0)
    let flags = header.u32(4)
    let size = Int(header.u32(8))
    let payload = try readFully(fd, count: size)
    let fds = parseFileDescriptors(control: control, controllen: controllen)
    return VhostMessage(request: request, flags: flags, payload: payload, fds: fds)
}

private func parseFileDescriptors(control: Data, controllen: Int) -> [Int32] {
    var fds: [Int32] = []
    let headerSize = MemoryLayout<cmsghdr>.size
    let alignment = MemoryLayout<Int>.alignment
    var offset = 0
    while offset + headerSize <= controllen {
        let cmsg = control.withUnsafeBytes { raw in
            raw.loadUnaligned(fromByteOffset: offset, as: cmsghdr.self)
        }
        let length = Int(cmsg.cmsg_len)
        if length < headerSize || offset + length > controllen {
            break
        }
        if cmsg.cmsg_level == SOL_SOCKET && cmsg.cmsg_type == SCM_RIGHTS {
            let dataOffset = offset + headerSize
            let dataLength = length - headerSize
            let count = dataLength / MemoryLayout<Int32>.size
            for i in 0..<count {
                let fd = control.withUnsafeBytes { raw in
                    raw.loadUnaligned(fromByteOffset: dataOffset + i * MemoryLayout<Int32>.size, as: Int32.self)
                }
                fds.append(fd)
            }
        }
        offset = (offset + length + alignment - 1) & ~(alignment - 1)
    }
    return fds
}

private func sendReply(_ fd: Int32, request: UInt32, payload: Data = Data()) throws {
    var data = Data()
    appendU32(&data, request)
    appendU32(&data, vhostUserVersion | vhostUserReply)
    appendU32(&data, UInt32(payload.count))
    data.append(payload)
    try writeFully(fd, data)
}

private func closeFds(_ fds: [Int32]) {
    for fd in fds {
        close(fd)
    }
}

private func serve(socketPath: String, queues: Int) throws {
    unlink(socketPath)
    let listener = socket(AF_UNIX, SOCK_STREAM, 0)
    guard listener >= 0 else {
        throw RuntimeError("socket failed: \(String(cString: strerror(errno)))")
    }
    defer {
        close(listener)
        unlink(socketPath)
    }

    var address = sockaddr_un()
    address.sun_family = sa_family_t(AF_UNIX)
    try socketPath.withCString { path in
        let maxPath = MemoryLayout.size(ofValue: address.sun_path)
        guard strlen(path) < maxPath else { throw RuntimeError("socket path too long") }
        withUnsafeMutableBytes(of: &address.sun_path) { raw in
            raw.copyMemory(from: UnsafeRawBufferPointer(start: path, count: strlen(path) + 1))
        }
    }

    let bindResult = withUnsafePointer(to: &address) { pointer in
        pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockaddrPointer in
            bind(listener, sockaddrPointer, socklen_t(MemoryLayout<sockaddr_un>.size))
        }
    }
    guard bindResult == 0 else {
        throw RuntimeError("bind \(socketPath) failed: \(String(cString: strerror(errno)))")
    }
    guard listen(listener, 1) == 0 else {
        throw RuntimeError("listen failed: \(String(cString: strerror(errno)))")
    }

    log("[vhost-video-vt] listening on \(socketPath)")
    let conn = accept(listener, nil, nil)
    guard conn >= 0 else {
        throw RuntimeError("accept failed: \(String(cString: strerror(errno)))")
    }
    defer { close(conn) }
    log("[vhost-video-vt] QEMU connected")

    let backend = VideoBackend(queueCount: queues)
    while true {
        var pollFds = [pollfd(fd: conn, events: Int16(POLLIN), revents: 0)]
        var kickMap: [Int32: Int] = [:]
        for queue in backend.queues {
            if let kickFd = queue.kickFd {
                kickMap[kickFd] = queue.index
                pollFds.append(pollfd(fd: kickFd, events: Int16(POLLIN), revents: 0))
            }
        }

        let rc = poll(&pollFds, nfds_t(pollFds.count), -1)
        if rc < 0 {
            if errno == EINTR { continue }
            throw RuntimeError("poll failed: \(String(cString: strerror(errno)))")
        }

        for pollFd in pollFds where pollFd.revents & Int16(POLLIN) != 0 {
            if pollFd.fd != conn {
                if let queueIndex = kickMap[pollFd.fd] {
                    try backend.processKick(queueIndex: queueIndex)
                }
                continue
            }

            guard let message = try recvMessage(conn) else {
                log("[vhost-video-vt] QEMU disconnected")
                return
            }
            try handle(message: message, conn: conn, backend: backend, queues: queues)
        }
    }
}

private func handle(message: VhostMessage, conn: Int32, backend: VideoBackend, queues: Int) throws {
    switch message.request {
    case vhostUserGetFeatures:
        var payload = Data()
        appendU64(
            &payload,
            (1 << vhostUserFProtocolFeatures)
                | (1 << virtioFVersion1)
                | (1 << virtioVideoFResourceGuestPages)
        )
        try sendReply(conn, request: message.request, payload: payload)
        closeFds(message.fds)
    case vhostUserSetFeatures:
        log("[vhost-video-vt] features=0x\(String(message.payload.u64(0), radix: 16))")
        closeFds(message.fds)
    case vhostUserGetProtocolFeatures:
        var payload = Data()
        appendU64(&payload, (1 << vhostUserProtocolFMq) | (1 << vhostUserProtocolFConfig))
        try sendReply(conn, request: message.request, payload: payload)
        closeFds(message.fds)
    case vhostUserSetProtocolFeatures:
        log("[vhost-video-vt] protocol_features=0x\(String(message.payload.u64(0), radix: 16))")
        closeFds(message.fds)
    case vhostUserGetQueueNum:
        var payload = Data()
        appendU64(&payload, UInt64(queues))
        try sendReply(conn, request: message.request, payload: payload)
        closeFds(message.fds)
    case vhostUserSetMemTable:
        try backend.memory.setRegions(payload: message.payload, fds: message.fds)
    case vhostUserSetVringNum:
        let index = Int(message.payload.u32(0))
        try backend.queue(index).size = message.payload.u32(4)
        closeFds(message.fds)
    case vhostUserSetVringAddr:
        let index = Int(message.payload.u32(0))
        let queue = try backend.queue(index)
        queue.descAddress = message.payload.u64(8)
        queue.usedAddress = message.payload.u64(16)
        queue.availAddress = message.payload.u64(24)
        closeFds(message.fds)
    case vhostUserSetVringBase:
        let index = Int(message.payload.u32(0))
        try backend.queue(index).lastAvailIndex = UInt16(truncatingIfNeeded: message.payload.u32(4))
        closeFds(message.fds)
    case vhostUserGetVringBase:
        let index = message.payload.count >= 4 ? Int(message.payload.u32(0)) : 0
        let queue = try backend.queue(index)
        var payload = Data()
        appendU32(&payload, UInt32(index))
        appendU32(&payload, UInt32(queue.lastAvailIndex))
        try sendReply(conn, request: message.request, payload: payload)
        closeFds(message.fds)
    case vhostUserSetVringKick:
        let index = message.payload.count >= 8 ? Int(message.payload.u64(0)) : 0
        let queue = try backend.queue(index)
        if let old = queue.kickFd { close(old) }
        queue.kickFd = message.fds.first
        closeFds(Array(message.fds.dropFirst()))
    case vhostUserSetVringCall:
        let index = message.payload.count >= 8 ? Int(message.payload.u64(0)) : 0
        let queue = try backend.queue(index)
        if let old = queue.callFd { close(old) }
        queue.callFd = message.fds.first
        closeFds(Array(message.fds.dropFirst()))
    case vhostUserSetVringEnable:
        let index = Int(message.payload.u32(0))
        try backend.queue(index).enabled = message.payload.u32(4) != 0
        closeFds(message.fds)
    case vhostUserGetStatus:
        var payload = Data()
        appendU64(&payload, 0)
        try sendReply(conn, request: message.request, payload: payload)
        closeFds(message.fds)
    case vhostUserGetConfig:
        let offset = Int(message.payload.u32(0))
        let size = Int(message.payload.u32(4))
        let configFlags = message.payload.u32(8)
        var config = Data()
        appendU32(&config, 1)
        appendU32(&config, 40)
        appendU32(&config, 4096)
        let name = Array("scarlet-videotoolbox-decoder".utf8)
        config.append(contentsOf: name)
        config.append(Data(count: max(0, 32 - name.count)))
        var payload = Data()
        appendU32(&payload, UInt32(offset))
        appendU32(&payload, UInt32(size))
        appendU32(&payload, configFlags)
        if offset < config.count {
            payload.append(config.subdata(in: offset..<min(config.count, offset + size)))
        }
        if payload.count < 12 + size {
            payload.append(Data(count: 12 + size - payload.count))
        }
        try sendReply(conn, request: message.request, payload: payload)
        closeFds(message.fds)
    case vhostUserSetOwner,
         vhostUserResetOwner,
         vhostUserSetVringErr,
         vhostUserSetConfig,
         vhostUserResetDevice,
         vhostUserSetStatus:
        closeFds(message.fds)
    default:
        if message.flags & vhostUserNeedReply != 0 {
            try sendReply(conn, request: message.request)
        }
        closeFds(message.fds)
        log("[vhost-video-vt] ignored request \(message.request)")
    }
}

private func parseArguments() -> (String, Int) {
    var socketPath = "/private/tmp/scarlet-video.sock"
    var queues = 2
    var index = 1
    let args = CommandLine.arguments
    while index < args.count {
        if args[index] == "--socket", index + 1 < args.count {
            socketPath = args[index + 1]
            index += 2
        } else if args[index] == "--queues", index + 1 < args.count {
            queues = Int(args[index + 1]) ?? queues
            index += 2
        } else {
            index += 1
        }
    }
    return (socketPath, queues)
}

let (socketPath, queues) = parseArguments()
do {
    try serve(socketPath: socketPath, queues: queues)
} catch {
    fputs("[vhost-video-vt] fatal: \(error)\n", stderr)
    exit(1)
}
