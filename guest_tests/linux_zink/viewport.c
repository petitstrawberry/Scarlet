/* Maintenance1 must invert actual GPU pixels; trimming must retain commands. */
#define VK_NO_PROTOTYPES
#include "../linux_vulkan/loader.h"
#include "viewport-vert.h"
#include "viewport-frag.h"
#define SIZE 64
#define BYTES (SIZE * SIZE * 4)
#define FUNCTIONS(X) \
 X(vkCreateInstance) X(vkDestroyInstance) X(vkEnumeratePhysicalDevices) \
 X(vkGetPhysicalDeviceProperties) X(vkGetPhysicalDeviceMemoryProperties) X(vkGetPhysicalDeviceFormatProperties) \
 X(vkGetPhysicalDeviceImageFormatProperties) X(vkCreateDevice) X(vkDestroyDevice) X(vkGetDeviceQueue) \
 X(vkCreateBuffer) X(vkDestroyBuffer) X(vkGetBufferMemoryRequirements) X(vkAllocateMemory) X(vkFreeMemory) \
 X(vkBindBufferMemory) X(vkMapMemory) X(vkUnmapMemory) X(vkCreateImage) X(vkDestroyImage) \
 X(vkGetImageMemoryRequirements) X(vkBindImageMemory) X(vkCreateImageView) X(vkDestroyImageView) \
 X(vkCreateRenderPass) X(vkDestroyRenderPass) X(vkCreateFramebuffer) X(vkDestroyFramebuffer) \
 X(vkCreateShaderModule) X(vkDestroyShaderModule) X(vkCreatePipelineLayout) X(vkDestroyPipelineLayout) \
 X(vkCreateGraphicsPipelines) X(vkDestroyPipeline) X(vkCreateCommandPool) X(vkDestroyCommandPool) \
 X(vkAllocateCommandBuffers) X(vkResetCommandBuffer) X(vkBeginCommandBuffer) X(vkEndCommandBuffer) \
 X(vkCmdBeginRenderPass) X(vkCmdEndRenderPass) X(vkCmdBindPipeline) X(vkCmdSetViewport) X(vkCmdDraw) \
 X(vkCmdPipelineBarrier) X(vkCmdCopyImageToBuffer) X(vkQueueSubmit) X(vkQueueWaitIdle) X(vkGetDeviceProcAddr)
FUNCTIONS(DECLARE)
DECLARE(vkTrimCommandPoolKHR)
#define REQUIRE(x) do { if (!(x)) { fprintf(stderr,"failed: %s\n",#x); return 1; } } while (0)
static uint32_t memory_type(VkPhysicalDeviceMemoryProperties *m, uint32_t bits, VkMemoryPropertyFlags flags) {
 for (uint32_t i=0;i<m->memoryTypeCount;i++)
  if ((bits & (1u<<i)) && (m->memoryTypes[i].propertyFlags & flags)==flags) return i;
 fprintf(stderr,"no compatible memory\n"); exit(1);
}
DECLARE(vkCreateRenderPass2KHR)
DECLARE(vkCmdBeginRenderPass2KHR)
DECLARE(vkCmdEndRenderPass2KHR)
int main(int argc, char **argv) {
 int modern = argc == 2 && !strcmp(argv[1],"--renderpass2");
 setvbuf(stdout,NULL,_IONBF,0);
 if (load_vulkan()) return 1;
 FUNCTIONS(LOAD)
 const char *query = VK_KHR_GET_PHYSICAL_DEVICE_PROPERTIES_2_EXTENSION_NAME;
 VkInstance instance; VkInstanceCreateInfo ii={.sType=VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO,.enabledExtensionCount=1,.ppEnabledExtensionNames=&query};
 CHECK(vkCreateInstance(&ii,NULL,&instance));
 uint32_t count=1; VkPhysicalDevice physical;
 CHECK(vkEnumeratePhysicalDevices(instance,&count,&physical)); REQUIRE(count);
 VkPhysicalDeviceProperties props; vkGetPhysicalDeviceProperties(physical,&props); printf("GPU: %s\n",props.deviceName);
 VkFormatProperties fp; vkGetPhysicalDeviceFormatProperties(physical,VK_FORMAT_R8G8B8A8_UNORM,&fp);
 REQUIRE((fp.optimalTilingFeatures & (VK_FORMAT_FEATURE_TRANSFER_SRC_BIT | VK_FORMAT_FEATURE_TRANSFER_DST_BIT)) ==
   (VK_FORMAT_FEATURE_TRANSFER_SRC_BIT | VK_FORMAT_FEATURE_TRANSFER_DST_BIT));
 VkImageFormatProperties unsupported;
 REQUIRE(vkGetPhysicalDeviceImageFormatProperties(physical,VK_FORMAT_R8G8B8A8_UNORM,VK_IMAGE_TYPE_3D,
   VK_IMAGE_TILING_OPTIMAL,VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT,VK_IMAGE_CREATE_2D_ARRAY_COMPATIBLE_BIT,&unsupported)==VK_ERROR_FORMAT_NOT_SUPPORTED);
 float priority=1; VkDeviceQueueCreateInfo qi={.sType=VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO,.queueFamilyIndex=0,.queueCount=1,.pQueuePriorities=&priority};
 const char *extensions[]={VK_KHR_MAINTENANCE1_EXTENSION_NAME,VK_KHR_MAINTENANCE2_EXTENSION_NAME,VK_KHR_MULTIVIEW_EXTENSION_NAME,VK_KHR_CREATE_RENDERPASS_2_EXTENSION_NAME,VK_KHR_IMAGELESS_FRAMEBUFFER_EXTENSION_NAME,VK_KHR_IMAGE_FORMAT_LIST_EXTENSION_NAME};
 VkPhysicalDeviceImagelessFramebufferFeatures ima={.sType=VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_IMAGELESS_FRAMEBUFFER_FEATURES,.imagelessFramebuffer=VK_TRUE};
 VkDeviceCreateInfo di={.sType=VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO,.queueCreateInfoCount=1,.pQueueCreateInfos=&qi,.enabledExtensionCount=modern?6:1,.ppEnabledExtensionNames=extensions,.pNext=modern?&ima:NULL};
 VkDevice device; CHECK(vkCreateDevice(physical,&di,NULL,&device));
 PFN_vkVoidFunction trim=vkGetDeviceProcAddr(device,"vkTrimCommandPoolKHR"); memcpy(&vkTrimCommandPoolKHR,&trim,sizeof(trim)); REQUIRE(vkTrimCommandPoolKHR);
 if (modern) {
#define MODERN_LOAD(n) do { PFN_vkVoidFunction f=vkGetDeviceProcAddr(device,#n); memcpy(&n,&f,sizeof(n)); REQUIRE(n); } while(0)
  MODERN_LOAD(vkCreateRenderPass2KHR); MODERN_LOAD(vkCmdBeginRenderPass2KHR); MODERN_LOAD(vkCmdEndRenderPass2KHR);
 }
 VkQueue queue; vkGetDeviceQueue(device,0,0,&queue);
 VkPhysicalDeviceMemoryProperties mp; vkGetPhysicalDeviceMemoryProperties(physical,&mp);
 VkImageCreateInfo ici={.sType=VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO,.imageType=VK_IMAGE_TYPE_2D,.format=VK_FORMAT_R8G8B8A8_UNORM,
  .extent={SIZE,SIZE,1},.mipLevels=1,.arrayLayers=1,.samples=VK_SAMPLE_COUNT_1_BIT,.tiling=VK_IMAGE_TILING_OPTIMAL,
  .usage=VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT | VK_IMAGE_USAGE_TRANSFER_SRC_BIT,.sharingMode=VK_SHARING_MODE_EXCLUSIVE,.initialLayout=VK_IMAGE_LAYOUT_UNDEFINED};
 VkImage image; CHECK(vkCreateImage(device,&ici,NULL,&image));
 VkMemoryRequirements req; vkGetImageMemoryRequirements(device,image,&req);
 VkMemoryAllocateInfo ai={.sType=VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,.allocationSize=req.size,.memoryTypeIndex=memory_type(&mp,req.memoryTypeBits,0)};
 VkDeviceMemory im; CHECK(vkAllocateMemory(device,&ai,NULL,&im)); CHECK(vkBindImageMemory(device,image,im,0));
 VkImageSubresourceRange range={.aspectMask=VK_IMAGE_ASPECT_COLOR_BIT,.levelCount=1,.layerCount=1};
 VkImageViewCreateInfo vi={.sType=VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO,.image=image,.viewType=VK_IMAGE_VIEW_TYPE_2D,.format=ici.format,.subresourceRange=range};
 VkImageView view; CHECK(vkCreateImageView(device,&vi,NULL,&view));
 VkAttachmentDescription attachment={.format=ici.format,.samples=VK_SAMPLE_COUNT_1_BIT,.loadOp=VK_ATTACHMENT_LOAD_OP_CLEAR,
  .storeOp=VK_ATTACHMENT_STORE_OP_STORE,.stencilLoadOp=VK_ATTACHMENT_LOAD_OP_DONT_CARE,.stencilStoreOp=VK_ATTACHMENT_STORE_OP_DONT_CARE,
  .initialLayout=VK_IMAGE_LAYOUT_UNDEFINED,.finalLayout=VK_IMAGE_LAYOUT_GENERAL};
 VkAttachmentReference color={.attachment=0,.layout=VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL};
 VkSubpassDescription subpass={.pipelineBindPoint=VK_PIPELINE_BIND_POINT_GRAPHICS,.colorAttachmentCount=1,.pColorAttachments=&color};
 VkRenderPassCreateInfo ri={.sType=VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO,.attachmentCount=1,.pAttachments=&attachment,.subpassCount=1,.pSubpasses=&subpass};
 VkRenderPass pass;
 if (modern) {
  VkAttachmentDescription2 a={.sType=VK_STRUCTURE_TYPE_ATTACHMENT_DESCRIPTION_2,.format=attachment.format,.samples=attachment.samples,.loadOp=attachment.loadOp,.storeOp=attachment.storeOp,.stencilLoadOp=attachment.stencilLoadOp,.stencilStoreOp=attachment.stencilStoreOp,.initialLayout=attachment.initialLayout,.finalLayout=attachment.finalLayout};
  VkAttachmentReference2 r={.sType=VK_STRUCTURE_TYPE_ATTACHMENT_REFERENCE_2,.attachment=0,.layout=color.layout};
  VkSubpassDescription2 sp={.sType=VK_STRUCTURE_TYPE_SUBPASS_DESCRIPTION_2,.pipelineBindPoint=VK_PIPELINE_BIND_POINT_GRAPHICS,.colorAttachmentCount=1,.pColorAttachments=&r};
  VkRenderPassCreateInfo2 info={.sType=VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO_2,.attachmentCount=1,.pAttachments=&a,.subpassCount=1,.pSubpasses=&sp};
  CHECK(vkCreateRenderPass2KHR(device,&info,NULL,&pass));
 } else CHECK(vkCreateRenderPass(device,&ri,NULL,&pass));
 VkFramebufferCreateInfo fi={.sType=VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO,.renderPass=pass,.attachmentCount=1,.pAttachments=&view,.width=SIZE,.height=SIZE,.layers=1};
 VkFramebufferAttachmentImageInfo fa={.sType=VK_STRUCTURE_TYPE_FRAMEBUFFER_ATTACHMENT_IMAGE_INFO,.usage=ici.usage,.width=SIZE,.height=SIZE,.layerCount=1,.viewFormatCount=1,.pViewFormats=&ici.format};
 VkFramebufferAttachmentsCreateInfo fas={.sType=VK_STRUCTURE_TYPE_FRAMEBUFFER_ATTACHMENTS_CREATE_INFO,.attachmentImageInfoCount=1,.pAttachmentImageInfos=&fa};
 if (modern) { fi.flags=VK_FRAMEBUFFER_CREATE_IMAGELESS_BIT; fi.pAttachments=NULL; fi.pNext=&fas; }
 VkFramebuffer framebuffer; CHECK(vkCreateFramebuffer(device,&fi,NULL,&framebuffer));
 VkShaderModuleCreateInfo smi={.sType=VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO,.codeSize=sizeof(viewport_vert),.pCode=viewport_vert};
 VkShaderModule vs,fs; CHECK(vkCreateShaderModule(device,&smi,NULL,&vs));
 smi.codeSize=sizeof(viewport_frag); smi.pCode=viewport_frag; CHECK(vkCreateShaderModule(device,&smi,NULL,&fs));
 VkPipelineShaderStageCreateInfo stages[2]={
  {.sType=VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO,.stage=VK_SHADER_STAGE_VERTEX_BIT,.module=vs,.pName="main"},
  {.sType=VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO,.stage=VK_SHADER_STAGE_FRAGMENT_BIT,.module=fs,.pName="main"}};
 VkPipelineLayoutCreateInfo li={.sType=VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO};
 VkPipelineLayout layout; CHECK(vkCreatePipelineLayout(device,&li,NULL,&layout));
 VkPipelineVertexInputStateCreateInfo vertex={.sType=VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO};
 VkPipelineInputAssemblyStateCreateInfo assembly={.sType=VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO,.topology=VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST};
 VkRect2D scissor={.extent={SIZE,SIZE}};
 VkPipelineViewportStateCreateInfo viewport_state={.sType=VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO,.viewportCount=1,.scissorCount=1,.pScissors=&scissor};
 VkPipelineRasterizationStateCreateInfo raster={.sType=VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO,.polygonMode=VK_POLYGON_MODE_FILL,.cullMode=VK_CULL_MODE_NONE,.frontFace=VK_FRONT_FACE_COUNTER_CLOCKWISE,.lineWidth=1};
 VkPipelineMultisampleStateCreateInfo samples={.sType=VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO,.rasterizationSamples=VK_SAMPLE_COUNT_1_BIT};
 VkPipelineColorBlendAttachmentState ba={.colorWriteMask=0xf};
 VkPipelineColorBlendStateCreateInfo blend={.sType=VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO,.attachmentCount=1,.pAttachments=&ba};
 VkDynamicState dynamic=VK_DYNAMIC_STATE_VIEWPORT;
 VkPipelineDynamicStateCreateInfo ds={.sType=VK_STRUCTURE_TYPE_PIPELINE_DYNAMIC_STATE_CREATE_INFO,.dynamicStateCount=1,.pDynamicStates=&dynamic};
 VkGraphicsPipelineCreateInfo pi={.sType=VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO,.stageCount=2,.pStages=stages,.pVertexInputState=&vertex,
  .pInputAssemblyState=&assembly,.pViewportState=&viewport_state,.pRasterizationState=&raster,.pMultisampleState=&samples,.pColorBlendState=&blend,
  .pDynamicState=&ds,.layout=layout,.renderPass=pass};
 VkPipeline pipeline; CHECK(vkCreateGraphicsPipelines(device,VK_NULL_HANDLE,1,&pi,NULL,&pipeline));
 VkBufferCreateInfo bi={.sType=VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO,.size=BYTES,.usage=VK_BUFFER_USAGE_TRANSFER_DST_BIT,.sharingMode=VK_SHARING_MODE_EXCLUSIVE};
 VkBuffer buffer; CHECK(vkCreateBuffer(device,&bi,NULL,&buffer)); vkGetBufferMemoryRequirements(device,buffer,&req);
 ai.allocationSize=req.size; ai.memoryTypeIndex=memory_type(&mp,req.memoryTypeBits,VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT|VK_MEMORY_PROPERTY_HOST_COHERENT_BIT);
 VkDeviceMemory bm; CHECK(vkAllocateMemory(device,&ai,NULL,&bm)); CHECK(vkBindBufferMemory(device,buffer,bm,0));
 VkCommandPoolCreateInfo pci={.sType=VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO,.flags=VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT,.queueFamilyIndex=0};
 VkCommandPool pool; CHECK(vkCreateCommandPool(device,&pci,NULL,&pool));
 VkCommandBufferAllocateInfo cai={.sType=VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO,.commandPool=pool,.level=VK_COMMAND_BUFFER_LEVEL_PRIMARY,.commandBufferCount=1};
 VkCommandBuffer command; CHECK(vkAllocateCommandBuffers(device,&cai,&command));
 unsigned char pixels[3][BYTES];
 for (unsigned iteration=0;iteration<3;iteration++) {
  printf("viewport iteration=%u\n",iteration);
  if (iteration) CHECK(vkResetCommandBuffer(command,0));
  VkCommandBufferBeginInfo cbi={.sType=VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO}; CHECK(vkBeginCommandBuffer(command,&cbi));
  VkClearValue clear={.color={{0,0,1,1}}};
  VkRenderPassBeginInfo rbi={.sType=VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO,.renderPass=pass,.framebuffer=framebuffer,.renderArea=scissor,.clearValueCount=1,.pClearValues=&clear};
  VkRenderPassAttachmentBeginInfo attachment_begin={.sType=VK_STRUCTURE_TYPE_RENDER_PASS_ATTACHMENT_BEGIN_INFO,.attachmentCount=1,.pAttachments=&view};
  VkSubpassBeginInfo begin={.sType=VK_STRUCTURE_TYPE_SUBPASS_BEGIN_INFO,.contents=VK_SUBPASS_CONTENTS_INLINE};
  if (modern) { rbi.pNext=&attachment_begin; vkCmdBeginRenderPass2KHR(command,&rbi,&begin); }
  else vkCmdBeginRenderPass(command,&rbi,VK_SUBPASS_CONTENTS_INLINE);
  vkCmdBindPipeline(command,VK_PIPELINE_BIND_POINT_GRAPHICS,pipeline);
  VkViewport vp={.width=SIZE,.height=iteration==0?SIZE:iteration==1?-SIZE:0,.y=iteration?SIZE:0,.maxDepth=1};
  vkCmdSetViewport(command,0,1,&vp); vkCmdDraw(command,3,1,0,0);
  if (modern) { VkSubpassEndInfo end={.sType=VK_STRUCTURE_TYPE_SUBPASS_END_INFO}; vkCmdEndRenderPass2KHR(command,&end); } else vkCmdEndRenderPass(command);
  VkImageMemoryBarrier barrier={.sType=VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER,.srcAccessMask=VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT,.dstAccessMask=VK_ACCESS_TRANSFER_READ_BIT,
   .oldLayout=VK_IMAGE_LAYOUT_GENERAL,.newLayout=VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,.srcQueueFamilyIndex=VK_QUEUE_FAMILY_IGNORED,.dstQueueFamilyIndex=VK_QUEUE_FAMILY_IGNORED,.image=image,.subresourceRange=range};
  vkCmdPipelineBarrier(command,VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT,VK_PIPELINE_STAGE_TRANSFER_BIT,0,0,NULL,0,NULL,1,&barrier);
  VkBufferImageCopy copy={.imageSubresource={VK_IMAGE_ASPECT_COLOR_BIT,0,0,1},.imageExtent={SIZE,SIZE,1}};
  vkCmdCopyImageToBuffer(command,image,VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,buffer,1,&copy);
  VkBufferMemoryBarrier host={.sType=VK_STRUCTURE_TYPE_BUFFER_MEMORY_BARRIER,.srcAccessMask=VK_ACCESS_TRANSFER_WRITE_BIT,.dstAccessMask=VK_ACCESS_HOST_READ_BIT,
   .srcQueueFamilyIndex=VK_QUEUE_FAMILY_IGNORED,.dstQueueFamilyIndex=VK_QUEUE_FAMILY_IGNORED,.buffer=buffer,.size=BYTES};
  vkCmdPipelineBarrier(command,VK_PIPELINE_STAGE_TRANSFER_BIT,VK_PIPELINE_STAGE_HOST_BIT,0,0,NULL,1,&host,0,NULL);
  CHECK(vkEndCommandBuffer(command)); vkTrimCommandPoolKHR(device,pool,0);
  VkSubmitInfo submit={.sType=VK_STRUCTURE_TYPE_SUBMIT_INFO,.commandBufferCount=1,.pCommandBuffers=&command};
  CHECK(vkQueueSubmit(queue,1,&submit,VK_NULL_HANDLE)); CHECK(vkQueueWaitIdle(queue));
  void *mapped; CHECK(vkMapMemory(device,bm,0,BYTES,0,&mapped)); memcpy(pixels[iteration],mapped,BYTES); vkUnmapMemory(device,bm);
 }
 unsigned green=0;
 for (unsigned y=0;y<SIZE;y++) for (unsigned x=0;x<SIZE;x++) {
  unsigned i=(y*SIZE+x)*4, flipped=((SIZE-1-y)*SIZE+x)*4;
  REQUIRE(memcmp(pixels[0]+i,pixels[1]+flipped,4)==0);
  REQUIRE(pixels[2][i]==0 && pixels[2][i+1]==0 && pixels[2][i+2]==255 && pixels[2][i+3]==255);
  if (pixels[0][i]==0 && pixels[0][i+1]==255 && pixels[0][i+2]==0 && pixels[0][i+3]==255) green++;
  else REQUIRE(pixels[0][i]==0 && pixels[0][i+1]==0 && pixels[0][i+2]==255 && pixels[0][i+3]==255);
 }
 REQUIRE(green>200 && green<1000);
 if (modern) printf("PASS RenderPass2 + imageless framebuffer GPU execution\n");
 printf("PASS maintenance1: %u triangle pixels, exact Y mirror, zero-height clear, trimmed executable commands\n",green);
 vkDestroyCommandPool(device,pool,NULL); vkDestroyBuffer(device,buffer,NULL); vkFreeMemory(device,bm,NULL);
 vkDestroyPipeline(device,pipeline,NULL); vkDestroyPipelineLayout(device,layout,NULL);
 vkDestroyShaderModule(device,vs,NULL); vkDestroyShaderModule(device,fs,NULL);
 vkDestroyFramebuffer(device,framebuffer,NULL); vkDestroyRenderPass(device,pass,NULL); vkDestroyImageView(device,view,NULL);
 vkDestroyImage(device,image,NULL); vkFreeMemory(device,im,NULL); vkDestroyDevice(device,NULL); vkDestroyInstance(instance,NULL);
 return 0;
}
