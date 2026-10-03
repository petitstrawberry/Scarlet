/* First Zink execution gate: a real offscreen GL context and pixel readback. */
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GL/gl.h>
#include <stdio.h>
#include <string.h>
#define GL_FUNCTIONS(X) X(glGetString) X(glViewport) X(glClearColor) X(glClear) X(glFinish) X(glReadPixels) X(glGetError) X(glBegin) X(glEnd) X(glColor4f) X(glVertex2f) X(glTexCoord2f) X(glGenTextures) X(glBindTexture) X(glTexParameteri) X(glTexImage2D) X(glEnable) X(glDisable) X(glDeleteTextures)
#define DECLARE_GL(name) static __typeof__(&name) p_##name;
GL_FUNCTIONS(DECLARE_GL)
#define glGetString p_glGetString
#define glViewport p_glViewport
#define glClearColor p_glClearColor
#define glClear p_glClear
#define glFinish p_glFinish
#define glReadPixels p_glReadPixels
#define glGetError p_glGetError
#define glBegin p_glBegin
#define glEnd p_glEnd
#define glColor4f p_glColor4f
#define glVertex2f p_glVertex2f
#define glTexCoord2f p_glTexCoord2f
#define glGenTextures p_glGenTextures
#define glBindTexture p_glBindTexture
#define glTexParameteri p_glTexParameteri
#define glTexImage2D p_glTexImage2D
#define glEnable p_glEnable
#define glDisable p_glDisable
#define glDeleteTextures p_glDeleteTextures
#define REQUIRE(x) do { if (!(x)) { fprintf(stderr,"failed: %s (EGL 0x%x)\n",#x,eglGetError()); return 1; } } while(0)
int main(int argc, char **argv) {
 int reference=argc==2 && !strcmp(argv[1],"--reference");
 const char *label=reference?"reference softpipe":"SGFX Zink";
 setvbuf(stdout,NULL,_IONBF,0);
 PFNEGLGETPLATFORMDISPLAYEXTPROC platform=(PFNEGLGETPLATFORMDISPLAYEXTPROC)eglGetProcAddress("eglGetPlatformDisplayEXT"); REQUIRE(platform);
 PFNEGLQUERYDEVICESEXTPROC devices_fn=(PFNEGLQUERYDEVICESEXTPROC)eglGetProcAddress("eglQueryDevicesEXT");
 PFNEGLQUERYDEVICESTRINGEXTPROC device_string=(PFNEGLQUERYDEVICESTRINGEXTPROC)eglGetProcAddress("eglQueryDeviceStringEXT"); REQUIRE(devices_fn && device_string);
 EGLDeviceEXT devices[16], transport=EGL_NO_DEVICE_EXT; EGLint device_count=0; REQUIRE(devices_fn(16,devices,&device_count));
 for(EGLint i=0;i<device_count;i++) { const char *exts=device_string(devices[i],EGL_EXTENSIONS); if(exts && strstr(exts,"EGL_MESA_device_software")) { transport=devices[i]; break; } }
 REQUIRE(transport!=EGL_NO_DEVICE_EXT);
 /* This EGL device supplies memory-backed pbuffer transport. Zink independently
  * selects the Vulkan GPU; the GL renderer assertion below checks that choice. */
 EGLDisplay display=platform(EGL_PLATFORM_DEVICE_EXT,transport,NULL); REQUIRE(display!=EGL_NO_DISPLAY);
 #define LOAD_GL(name) do { __eglMustCastToProperFunctionPointerType f=eglGetProcAddress(#name); memcpy(&p_##name,&f,sizeof(f)); REQUIRE(p_##name); } while(0);
 GL_FUNCTIONS(LOAD_GL)
 EGLint major,minor; REQUIRE(eglInitialize(display,&major,&minor)); printf("EGL %d.%d vendor: %s\n",major,minor,eglQueryString(display,EGL_VENDOR));
 REQUIRE(eglBindAPI(EGL_OPENGL_API));
 EGLint attributes[]={EGL_SURFACE_TYPE,EGL_PBUFFER_BIT,EGL_RENDERABLE_TYPE,EGL_OPENGL_BIT,EGL_RED_SIZE,8,EGL_GREEN_SIZE,8,EGL_BLUE_SIZE,8,EGL_ALPHA_SIZE,8,EGL_NONE};
 EGLConfig config; EGLint count; REQUIRE(eglChooseConfig(display,attributes,&config,1,&count)); REQUIRE(count==1);
 EGLint pbuffer[]={EGL_WIDTH,64,EGL_HEIGHT,64,EGL_NONE}; EGLSurface surface=eglCreatePbufferSurface(display,config,pbuffer); REQUIRE(surface!=EGL_NO_SURFACE);
 EGLint context_attributes[]={EGL_CONTEXT_MAJOR_VERSION,2,EGL_CONTEXT_MINOR_VERSION,1,EGL_NONE}; EGLContext context=eglCreateContext(display,config,EGL_NO_CONTEXT,context_attributes); REQUIRE(context!=EGL_NO_CONTEXT); REQUIRE(eglMakeCurrent(display,surface,surface,context));
 const char *renderer=(const char*)glGetString(GL_RENDERER); printf("GL renderer: %s\nGL version: %s\n",renderer,(const char*)glGetString(GL_VERSION)); REQUIRE(renderer && (reference ? strstr(renderer,"softpipe")!=NULL : strstr(renderer,"zink") && strstr(renderer,"SGFX")));
 glViewport(0,0,64,64); glClearColor(1,0,0,1); glClear(GL_COLOR_BUFFER_BIT); glFinish(); unsigned char pixels[64*64*4]; glReadPixels(0,0,64,64,GL_RGBA,GL_UNSIGNED_BYTE,pixels); REQUIRE(glGetError()==GL_NO_ERROR);
 for(unsigned i=0;i<64*64;i++) REQUIRE(pixels[4*i]==255 && pixels[4*i+1]==0 && pixels[4*i+2]==0 && pixels[4*i+3]==255);
 printf("PASS %s clear readback\n",label);
 glColor4f(0,1,0,1); glBegin(GL_TRIANGLES); glVertex2f(-0.75f,-0.75f); glVertex2f(0.75f,-0.75f); glVertex2f(0,0.75f); glEnd();
 glFinish(); glReadPixels(0,0,64,64,GL_RGBA,GL_UNSIGNED_BYTE,pixels); REQUIRE(glGetError()==GL_NO_ERROR);
 unsigned center=4*(32*64+32),corner=4*(1*64+1); REQUIRE(pixels[center]==0 && pixels[center+1]==255 && pixels[center+2]==0); REQUIRE(pixels[corner]==255 && pixels[corner+1]==0 && pixels[corner+2]==0);
 printf("PASS %s triangle readback\n",label);
 const unsigned char texels[]={255,0,0,255, 0,255,0,255, 0,0,255,255, 255,255,255,255}; GLuint texture;
 glGenTextures(1,&texture); glBindTexture(GL_TEXTURE_2D,texture); glTexParameteri(GL_TEXTURE_2D,GL_TEXTURE_MIN_FILTER,GL_NEAREST); glTexParameteri(GL_TEXTURE_2D,GL_TEXTURE_MAG_FILTER,GL_NEAREST); glTexImage2D(GL_TEXTURE_2D,0,GL_RGBA,2,2,0,GL_RGBA,GL_UNSIGNED_BYTE,texels);
 glEnable(GL_TEXTURE_2D); glColor4f(1,1,1,1); glBegin(GL_QUADS);
 glTexCoord2f(0,0); glVertex2f(-1,-1); glTexCoord2f(1,0); glVertex2f(1,-1); glTexCoord2f(1,1); glVertex2f(1,1); glTexCoord2f(0,1); glVertex2f(-1,1); glEnd(); glFinish(); glReadPixels(0,0,64,64,GL_RGBA,GL_UNSIGNED_BYTE,pixels); REQUIRE(glGetError()==GL_NO_ERROR);
 const unsigned positions[]={16*64+16,16*64+48,48*64+16,48*64+48};
 for(unsigned i=0;i<4;i++) REQUIRE(memcmp(pixels+4*positions[i],texels+4*i,4)==0);
 glDisable(GL_TEXTURE_2D); glDeleteTextures(1,&texture); printf("PASS %s texture readback\n",label);
 eglMakeCurrent(display,EGL_NO_SURFACE,EGL_NO_SURFACE,EGL_NO_CONTEXT); eglDestroyContext(display,context); eglDestroySurface(display,surface); eglTerminate(display); printf("PASS %s offscreen clear/triangle/texture\n",label); return 0;
}
